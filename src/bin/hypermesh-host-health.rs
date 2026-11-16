fn main() {
    #[cfg(target_os = "linux")]
    {
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        let (path, once) = match args.as_slice() {
            [path] => (path, false),
            [flag, path] if flag == "--once" => (path, true),
            _ => {
                eprintln!("usage: hypermesh-host-health [--once] /etc/hypermesh/health.json");
                std::process::exit(2);
            }
        };
        match hypermesh_visor::host_health::run(std::path::Path::new(path), once) {
            Ok(()) => return,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("host health authorization refused");
                std::process::exit(3);
            }
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionAborted => {
                eprintln!("host health authorization unavailable");
                std::process::exit(4);
            }
            Err(_) => {}
        }
    }
    eprintln!("host health unavailable");
    std::process::exit(1);
}
