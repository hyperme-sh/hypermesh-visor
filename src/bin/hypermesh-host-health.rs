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
        if hypermesh_visor::host_health::run(std::path::Path::new(path), once).is_ok() {
            return;
        }
    }
    eprintln!("host health unavailable");
    std::process::exit(1);
}
