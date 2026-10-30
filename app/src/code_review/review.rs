//! Local, read-only Git review. File rows share terminal metadata; only the
//! selected file owns an editor and performs detail I/O.
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use anyhow::{Context, Result};
use warp_editor::render::element::VerticalExpansionBehavior;
use warpui::r#async::SpawnedFutureHandle;
use warpui::elements::new_scrollable::SingleAxisConfig;
use warpui::elements::{
    ChildView, ClippedScrollStateHandle, ConstrainedBox, Container, Flex, Hoverable,
    MouseStateHandle, NewScrollable, Padding, ParentElement, Shrinkable, Text,
};
use warpui::platform::Cursor;
use warpui::{
    AppContext, Element, Entity, ModelHandle, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};

use super::diff_size_limits::{MAX_CHARACTERS_PER_LINE, MAX_DIFF_SIZE};
use super::diff_state::{DiffMode, GitFileStatus, LocalDiffStateModel};
use super::git_repo_model::{GitRepoStatusEvent, GitRepoStatusModel};
use crate::appearance::Appearance;
use crate::code::editor::view::{CodeEditorRenderOptions, CodeEditorView};
use crate::util::git::FileChangeEntry;
use crate::workspace::WorkspaceAction;

pub struct GitReviewView {
    root: PathBuf,
    status: ModelHandle<GitRepoStatusModel>,
    selected: Option<String>,
    files: Vec<FileChangeEntry>,
    editor: Option<ViewHandle<CodeEditorView>>,
    message: String,
    loading: Option<SpawnedFutureHandle>,
    pending: bool,
    selection_epoch: u64,
    expanded: bool,
    base: Option<String>,
    baseline_epoch: u64,
    list_scroll: ClippedScrollStateHandle,
    row_mouse: HashMap<String, MouseStateHandle>,
    close_mouse: MouseStateHandle,
    open_mouse: MouseStateHandle,
}

impl GitReviewView {
    pub fn new(
        root: PathBuf,
        status: ModelHandle<GitRepoStatusModel>,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        ctx.subscribe_to_model(&status, |me, _, event, ctx| match event {
            GitRepoStatusEvent::MetadataChanged => {
                if me.status.as_ref(ctx).metadata(ctx).is_none() {
                    me.message = "Unable to read repository changes".into();
                    ctx.notify();
                } else {
                    me.update_files(ctx);
                }
            }
            GitRepoStatusEvent::FilesInvalidated(changes) => {
                if changes.git_changed {
                    me.base = None;
                    me.baseline_epoch += 1;
                }
                if changes.full
                    || me
                        .selected
                        .as_ref()
                        .is_some_and(|path| changes.paths.contains(&PathBuf::from(path)))
                {
                    me.reload_selected(ctx);
                }
            }
        });
        let mut view = Self {
            root,
            status,
            selected: None,
            files: Vec::new(),
            editor: None,
            message: "Loading changes…".into(),
            loading: None,
            pending: false,
            selection_epoch: 0,
            expanded: true,
            base: None,
            baseline_epoch: 0,
            list_scroll: Default::default(),
            row_mouse: Default::default(),
            close_mouse: Default::default(),
            open_mouse: Default::default(),
        };
        view.update_files(ctx);
        view
    }

    fn update_files(&mut self, ctx: &mut ViewContext<Self>) {
        if self.status.as_ref(ctx).metadata(ctx).is_none() {
            return;
        }
        self.files = self
            .status
            .as_ref(ctx)
            .files(ctx)
            .unwrap_or_default()
            .to_vec();
        let paths: HashSet<_> = self.files.iter().map(|file| &file.path).collect();
        self.row_mouse.retain(|path, _| paths.contains(path));
        for file in &self.files {
            self.row_mouse.entry(file.path.clone()).or_default();
        }
        if !self
            .selected
            .as_ref()
            .is_some_and(|path| paths.contains(path))
        {
            self.select(self.files.first().map(|file| file.path.clone()), ctx);
        }
        if self.files.is_empty() {
            self.message = "No uncommitted changes".into();
        }
        ctx.notify();
    }

    fn select(&mut self, path: Option<String>, ctx: &mut ViewContext<Self>) {
        self.expanded = self.selected != path || !self.expanded;
        self.selection_epoch += 1;
        if let Some(task) = self.loading.take() {
            task.abort();
        }
        self.selected = path;
        self.base = None;
        self.editor = None;
        self.pending = false;
        self.reload_selected(ctx);
        ctx.notify();
    }

    fn reload_selected(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.expanded {
            return;
        }
        let Some(path) = self.selected.clone() else {
            return;
        };
        if self.loading.is_some() {
            self.pending = true;
            return;
        }
        self.message = "Updating…".into();
        let root = self.root.clone();
        let epoch = self.selection_epoch;
        let baseline_epoch = self.baseline_epoch;
        let base = self.base.clone();
        self.loading = Some(ctx.spawn(
            async move { load_file(root, path, base).await },
            move |me, result, ctx| {
                if me.selection_epoch != epoch {
                    return;
                }
                me.loading = None;
                if me.baseline_epoch != baseline_epoch {
                    me.pending = false;
                    me.reload_selected(ctx);
                    return;
                }
                match result {
                    Ok(Some((base, current))) => {
                        let editor = me.editor.get_or_insert_with(|| {
                            ctx.add_typed_action_view(|ctx| {
                                CodeEditorView::new(
                                    None,
                                    None,
                                    CodeEditorRenderOptions::new(
                                        VerticalExpansionBehavior::FillMaxHeight,
                                    ),
                                    ctx,
                                )
                            })
                        });
                        editor.update(ctx, |editor, ctx| {
                            editor.set_review_content(&base, &current, ctx)
                        });
                        me.base = Some(base);
                        me.message.clear();
                    }
                    Ok(None) => {
                        me.editor = None;
                        me.message = "No text diff to display".into();
                    }
                    Err(error) => {
                        me.editor = None;
                        me.message = format!("Unable to load diff: {error:#}");
                    }
                }
                if std::mem::take(&mut me.pending) {
                    me.reload_selected(ctx);
                }
                ctx.notify();
            },
        ));
        ctx.notify();
    }
}

async fn load_file(
    root: PathBuf,
    path: String,
    cached_base: Option<String>,
) -> Result<Option<(String, String)>> {
    // The editor already computes and renders the diff. Querying a Git patch
    // and its binary numstat first repeats that work and adds WSL launches.
    let Some((_, status)) =
        LocalDiffStateModel::file_status_for_path(&root, &path, &DiffMode::Head, None).await?
    else {
        return Ok(None);
    };
    let base = match (cached_base, &status) {
        (Some(base), _) => base,
        (_, GitFileStatus::New | GitFileStatus::Untracked) => String::new(),
        (_, status) => {
            let base_path = match status {
                GitFileStatus::Renamed { old_path } | GitFileStatus::Copied { old_path } => {
                    old_path
                }
                _ => &path,
            };
            warp_util::git::run_git_command(&root, &["show", &format!("HEAD:{base_path}")])
                .await
                .context("cannot read HEAD content")?
        }
    };
    if base.len() > MAX_DIFF_SIZE {
        anyhow::bail!("base content is too large to display");
    }
    let current = if matches!(status, GitFileStatus::Deleted) {
        String::new()
    } else {
        let file = root.join(path);
        if async_fs::metadata(&file).await?.len() > MAX_DIFF_SIZE as u64 {
            anyhow::bail!("file is too large to display");
        }
        async_fs::read_to_string(file).await?
    };
    if base.contains('\0') || current.contains('\0') {
        anyhow::bail!("binary file; open it in an external editor");
    }
    if base
        .lines()
        .chain(current.lines())
        .any(|line| line.chars().count() > MAX_CHARACTERS_PER_LINE)
    {
        anyhow::bail!("line is too long to display; open it in an external editor");
    }
    Ok(Some((base, current)))
}

#[derive(Debug, Clone)]
pub enum GitReviewAction {
    Select(String),
}

impl TypedActionView for GitReviewView {
    type Action = GitReviewAction;
    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            GitReviewAction::Select(path) => self.select(Some(path.clone()), ctx),
        }
    }
}

impl Entity for GitReviewView {
    type Event = ();
}
impl View for GitReviewView {
    fn ui_name() -> &'static str {
        "GitReview"
    }
    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let font = appearance.ui_font_family();
        let text_color = theme.main_text_color(theme.background()).into();
        let label = move |text: String| {
            Text::new_inline(text, font, 13.)
                .with_color(text_color)
                .finish()
        };
        let mut header = Flex::row();
        header
            .add_child(Shrinkable::new(1., label("Changes · HEAD → Working tree".into())).finish());
        header.add_child(
            Hoverable::new(self.close_mouse.clone(), move |_| {
                Container::new(label("Close".into()))
                    .with_padding(Padding::uniform(8.))
                    .finish()
            })
            .on_click(|ctx, _, _| {
                ctx.dispatch_typed_action(WorkspaceAction::CloseGitReview);
            })
            .with_cursor(Cursor::PointingHand)
            .finish(),
        );
        let mut rows = Flex::column();
        for file in &self.files {
            let path = file.path.clone();
            let selected = self.selected.as_ref() == Some(&path);
            let row = label(format!(
                "{}  {}   +{} −{}",
                if selected && self.expanded {
                    "▾"
                } else {
                    "▸"
                },
                path,
                file.additions,
                file.deletions
            ));
            rows.add_child(
                Hoverable::new(
                    self.row_mouse.get(&path).cloned().unwrap_or_default(),
                    move |_| {
                        Container::new(row)
                            .with_padding(Padding::uniform(6.))
                            .finish()
                    },
                )
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(GitReviewAction::Select(path.clone()));
                })
                .with_cursor(Cursor::PointingHand)
                .finish(),
            );
        }
        let list = NewScrollable::vertical(
            SingleAxisConfig::Clipped {
                handle: self.list_scroll.clone(),
                child: rows.finish(),
            },
            theme.disabled_text_color(theme.background()).into(),
            theme.main_text_color(theme.background()).into(),
            warpui::elements::Fill::None,
        )
        .finish();
        let mut column = Flex::column();
        column.add_child(header.finish());
        column.add_child(ConstrainedBox::new(list).with_height(180.).finish());
        if let Some(path) = &self.selected {
            let root = self.root.clone();
            let path = path.clone();
            column.add_child(
                Hoverable::new(self.open_mouse.clone(), move |_| {
                    Container::new(label("Open file ↗".into()))
                        .with_padding(Padding::uniform(6.))
                        .finish()
                })
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(WorkspaceAction::OpenFilePath {
                        path: root.join(&path),
                    });
                })
                .with_cursor(Cursor::PointingHand)
                .finish(),
            );
        }
        if !self.message.is_empty() {
            column.add_child(label(self.message.clone()));
        }
        if let Some(editor) = &self.editor {
            column.add_child(Shrinkable::new(1., ChildView::new(editor).finish()).finish());
        }
        Container::new(column.finish())
            .with_padding(Padding::uniform(8.))
            .finish()
    }
}

impl Drop for GitReviewView {
    fn drop(&mut self) {
        if let Some(task) = self.loading.take() {
            task.abort();
        }
    }
}
