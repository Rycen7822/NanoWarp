use crate::code::ProgrammingLanguage;
use crate::terminal::shell::ShellType;

#[test]
fn test_programming_language_from_string() {
    // Shell language specifiers should produce Shell variants
    assert_eq!(
        ProgrammingLanguage::from("bash".to_string()),
        ProgrammingLanguage::Shell(ShellType::Bash)
    );
    assert_eq!(
        ProgrammingLanguage::from("shell".to_string()),
        ProgrammingLanguage::Shell(ShellType::Bash)
    );
    assert_eq!(
        ProgrammingLanguage::from("sh".to_string()),
        ProgrammingLanguage::Shell(ShellType::Bash)
    );
    assert_eq!(
        ProgrammingLanguage::from("zsh".to_string()),
        ProgrammingLanguage::Shell(ShellType::Zsh)
    );
    assert_eq!(
        ProgrammingLanguage::from("fish".to_string()),
        ProgrammingLanguage::Shell(ShellType::Fish)
    );
    assert_eq!(
        ProgrammingLanguage::from("powershell".to_string()),
        ProgrammingLanguage::Shell(ShellType::PowerShell)
    );
    assert_eq!(
        ProgrammingLanguage::from("pwsh".to_string()),
        ProgrammingLanguage::Shell(ShellType::PowerShell)
    );

    // Non-shell languages should produce Other variants
    assert_eq!(
        ProgrammingLanguage::from("python".to_string()),
        ProgrammingLanguage::Other("python".to_string())
    );
    assert_eq!(
        ProgrammingLanguage::from("rust".to_string()),
        ProgrammingLanguage::Other("rust".to_string())
    );
    assert_eq!(
        ProgrammingLanguage::from("javascript".to_string()),
        ProgrammingLanguage::Other("javascript".to_string())
    );
}

#[test]
fn test_programming_language_to_extension() {
    // Each entry is (markdown language token, expected extension). The expected extension
    // must resolve back to a recognized language via `languages::language_by_filename` so that
    // syntax highlighting is applied to the AI block.
    let cases: &[(&str, &str)] = &[
        // Canonical names.
        ("rust", "rs"),
        ("go", "go"),
        ("python", "py"),
        ("javascript", "js"),
        ("typescript", "ts"),
        ("yaml", "yaml"),
        ("cpp", "cpp"),
        ("java", "java"),
        ("c#", "cs"),
        ("csharp", "cs"),
        ("html", "html"),
        ("css", "css"),
        ("c", "c"),
        ("json", "json"),
        ("hcl", "hcl"),
        ("lua", "lua"),
        ("ruby", "rb"),
        ("php", "php"),
        ("toml", "toml"),
        ("swift", "swift"),
        ("kotlin", "kt"),
        ("powershell", "ps1"),
        ("elixir", "exs"),
        ("scala", "scala"),
        ("sql", "sql"),
        // Languages newly covered by this fix — previously fell through to None and rendered
        // without syntax highlighting in AI blocks even though the `languages` crate supports them.
        ("jsx", "jsx"),
        ("tsx", "tsx"),
        ("xml", "xml"),
        ("vue", "vue"),
        ("dockerfile", "dockerfile"),
        ("starlark", "bzl"),
        ("objective-c", "m"),
        ("objc", "m"),
        // Common markdown code-fence aliases.
        ("rs", "rs"),
        ("golang", "go"),
        ("py", "py"),
        ("js", "js"),
        ("ts", "ts"),
        ("yml", "yaml"),
        ("c++", "cpp"),
        ("rb", "rb"),
        ("kt", "kt"),
        ("terraform", "hcl"),
        ("tf", "hcl"),
        ("docker", "dockerfile"),
        ("containerfile", "dockerfile"),
        ("markdown", "md"),
        ("md", "md"),
    ];
    for (token, expected_extension) in cases {
        let language = ProgrammingLanguage::from((*token).to_string());
        assert_eq!(
            language.to_extension(),
            Some(*expected_extension),
            "expected to_extension({token:?}) to be Some({expected_extension:?})",
        );
    }

    // PowerShell remains the only Shell variant whose extension is exposed; this preserves
    // existing behavior for the other Shell variants which are intentionally not extended here.
    assert_eq!(
        ProgrammingLanguage::Shell(ShellType::PowerShell).to_extension(),
        Some("ps1"),
    );

    // Unrecognized tokens still return None.
    assert_eq!(
        ProgrammingLanguage::Other("definitely-not-a-language".to_string()).to_extension(),
        None,
    );
}
