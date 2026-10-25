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
