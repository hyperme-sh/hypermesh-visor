//! Linux package sidecar. The parent keeps the port reserved until shutdown.
use std::net::TcpListener;
use std::os::fd::OwnedFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub struct ManagedConductor {
    child: std::sync::Mutex<Child>,
    listener: TcpListener,
}

impl ManagedConductor {
    pub fn start(config: &std::ffi::OsStr, journal: &std::ffi::OsStr) -> Result<Self, String> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
        let inherited: OwnedFd = listener.try_clone().map_err(|e| e.to_string())?.into();
        let parent = std::process::id();
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let prefix = executable
            .parent()
            .and_then(|bin| bin.parent())
            .ok_or("invalid package layout")?;
        #[cfg(feature = "desktop")]
        let relative = "lib/hypermesh-visor/conductor";
        #[cfg(not(feature = "desktop"))]
        let relative = "lib/hypermesh-visor-headless/conductor";
        let mut command = Command::new(prefix.join(relative));
        command
            .arg("serve")
            .arg("--stdin-listener")
            .arg("--config")
            .arg(config)
            .arg("--journal")
            .arg(journal)
            .stdin(Stdio::from(inherited));
        // Only async-signal-safe syscalls between fork and exec. Check the parent
        // after installing the signal to cover death between fork and prctl.
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() as u32 != parent {
                    return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
                }
                Ok(())
            });
        }
        let child = command
            .spawn()
            .map_err(|e| format!("starting bundled Conductor: {e}"))?;
        let mut managed = Self {
            child: std::sync::Mutex::new(child),
            listener,
        };
        let agent = ureq::AgentBuilder::new()
            .try_proxy_from_env(false)
            .redirects(0)
            .timeout(Duration::from_millis(300))
            .build();
        let url = format!("http://{}/v1/models", managed.address()?);
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if managed
                .child
                .get_mut()
                .unwrap()
                .try_wait()
                .map_err(|e| e.to_string())?
                .is_some()
            {
                return Err("bundled Conductor exited during startup".into());
            }
            if agent.get(&url).call().is_ok() {
                return Ok(managed);
            }
            if Instant::now() >= deadline {
                return Err("bundled Conductor readiness timed out".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    pub async fn monitor(&self) -> std::io::Result<()> {
        loop {
            if let Some(status) = self.child.lock().unwrap().try_wait()? {
                return Err(std::io::Error::other(format!(
                    "bundled Conductor exited: {status}"
                )));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    pub fn address(&self) -> Result<String, String> {
        self.listener
            .local_addr()
            .map(|a| a.to_string())
            .map_err(|e| e.to_string())
    }
}

impl Drop for ManagedConductor {
    fn drop(&mut self) {
        let child = self.child.get_mut().unwrap();
        let _ = child.kill();
        let _ = child.wait();
    }
}
