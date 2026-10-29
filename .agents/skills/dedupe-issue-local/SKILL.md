---
name: dedupe-issue-local
description: Compare NanoWarp issue reports for an evidence-backed duplicate, using terminal, input, rendering, basic-editor and SSH context.
---

# NanoWarp issue deduplication

Use the current NanoWarp issue and candidate issue bodies, comments and
attachments. This guide is self-contained and requires no upstream parent
skill installation or scheduled maintenance loop.

## Compare the failing behavior

- Normalize low-signal prefixes such as `Bug:`, `[Feature]`, `Warp:` or
  `NanoWarp:` and platform tags for title comparison. Titles only nominate
  candidates; they do not prove a duplicate.
- Require the same observable symptom and compatible reproduction path.
  Compare expected behavior, trigger, regression timing and relevant source.
- For terminals, compare shell/session context, output, prompts, input and
  SSH/WSL/tmux involvement. For UI defects, compare the affected surface,
  focus/display state and visual symptom.
- Treat versions and platforms as supporting context. Do not merge distinct
  platform causes just because screenshots or titles look similar.
- Reports about retired AI, account, cloud, LSP or Notebook-preview features
  concern removed scope; matching an upstream commercial issue does not
  establish an active NanoWarp defect. External CLI use is still supported.

Return the candidate issue link, shared evidence, any material differences and
a duplicate/related/insufficient-evidence conclusion. Use labels from
`.github/issue-triage/config.json`. The `agent:*` labels describe development
triage and do not imply product Agent functionality. Propose mutations unless
the current task authorizes posting, labeling or closing issues.
