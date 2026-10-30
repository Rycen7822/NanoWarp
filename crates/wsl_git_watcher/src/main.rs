#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
fn main() {
    if let Err(error) = linux::run() {
        eprintln!("nanowarp-wsl-watch: {error:#}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("nanowarp-wsl-watch runs inside Linux only");
    std::process::exit(1);
}
