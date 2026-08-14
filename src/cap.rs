//! The single place where this program touches the outside world.
//!
//! Every filesystem, process, and network effect in `legacy` is funnelled
//! through one of the capability modules below. Nothing else in the crate
//! references `std::fs`, `std::process`, or `std::net` directly, so the
//! program's entire blast radius is auditable by reading this one file.
//! The `// strictrs: capability` markers are what let strictrs verify that
//! property mechanically rather than on trust.

// strictrs: capability
pub mod fs {
    use std::fs::File;
    use std::io;
    use std::path::{Path, PathBuf};

    /// Read a UTF-8 file.
    pub fn read_to_string(path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    /// Read a file as raw bytes. Used for media, which is never decoded.
    pub fn read(path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    /// Write UTF-8, creating parent directories first.
    pub fn write(path: &Path, contents: &str) -> io::Result<()> {
        create_parent(path)?;
        std::fs::write(path, contents)
    }

    /// Write raw bytes, creating parent directories first.
    pub fn write_bytes(path: &Path, contents: &[u8]) -> io::Result<()> {
        create_parent(path)?;
        std::fs::write(path, contents)
    }

    pub fn create_dir_all(path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }

    fn create_parent(path: &Path) -> io::Result<()> {
        match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => std::fs::create_dir_all(parent),
            Some(_) | None => Ok(()),
        }
    }

    pub fn exists(path: &Path) -> bool {
        path.exists()
    }

    pub fn is_file(path: &Path) -> bool {
        path.is_file()
    }

    pub fn is_dir(path: &Path) -> bool {
        path.is_dir()
    }

    /// Copy a file verbatim, preserving its bytes exactly.
    pub fn copy(source: &Path, destination: &Path) -> io::Result<u64> {
        create_parent(destination)?;
        std::fs::copy(source, destination)
    }

    pub fn remove_file(path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }

    pub fn remove_dir_all(path: &Path) -> io::Result<()> {
        std::fs::remove_dir_all(path)
    }

    pub fn file_size(path: &Path) -> io::Result<u64> {
        Ok(std::fs::metadata(path)?.len())
    }

    pub fn open(path: &Path) -> io::Result<File> {
        std::fs::File::open(path)
    }

    pub fn create(path: &Path) -> io::Result<File> {
        create_parent(path)?;
        std::fs::File::create(path)
    }

    /// Directory entries, sorted, so every traversal is deterministic.
    pub fn read_dir_sorted(path: &Path) -> io::Result<Vec<PathBuf>> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(path)? {
            entries.push(entry?.path());
        }
        entries.sort();
        Ok(entries)
    }

    /// Every file under `root`, recursively, sorted by path.
    pub fn walk_files(root: &Path) -> io::Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        for entry in walkdir::WalkDir::new(root).sort_by_file_name() {
            let entry = entry.map_err(io::Error::other)?;
            if entry.file_type().is_file() {
                files.push(entry.path().to_path_buf());
            }
        }
        files.sort();
        Ok(files)
    }

    /// Mark a file executable on Unix. A no-op elsewhere.
    #[cfg(unix)]
    pub fn set_executable(path: &Path) -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions)
    }

    #[cfg(not(unix))]
    pub fn set_executable(_path: &Path) -> io::Result<()> {
        Ok(())
    }

    /// A scratch directory that deletes itself when dropped. Used while
    /// sealing; never becomes part of an archive.
    pub fn temp_dir() -> io::Result<tempfile::TempDir> {
        tempfile::TempDir::new()
    }
}

// strictrs: capability
pub mod process {
    use std::io;
    use std::path::Path;
    use std::process::{Command, Stdio};

    /// The outcome of running an external tool.
    pub struct Output {
        pub success: bool,
        pub stdout: String,
        pub stderr: String,
    }

    /// Run a program to completion, capturing its output.
    pub fn run(program: &str, args: &[&str]) -> io::Result<Output> {
        collect(Command::new(program).args(args).output()?)
    }

    /// Run a program with a specific working directory.
    pub fn run_in(directory: &Path, program: &str, args: &[&str]) -> io::Result<Output> {
        collect(
            Command::new(program)
                .args(args)
                .current_dir(directory)
                .output()?,
        )
    }

    fn collect(output: std::process::Output) -> io::Result<Output> {
        Ok(Output {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    /// A child process whose stdin stays open, so it can be stopped on a
    /// keystroke. Used only for push-to-talk recording with `ffmpeg`.
    pub struct Recording {
        child: std::process::Child,
    }

    impl Recording {
        /// Send `quit` to the child's stdin and wait for it to exit.
        pub fn stop(mut self, quit_key: &[u8]) -> io::Result<bool> {
            use std::io::Write;
            if let Some(mut stdin) = self.child.stdin.take() {
                stdin.write_all(quit_key)?;
                stdin.flush()?;
            }
            Ok(self.child.wait()?.success())
        }
    }

    pub fn spawn_quiet(program: &str, args: &[&str]) -> io::Result<Recording> {
        let child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Recording { child })
    }

    /// Whether an executable is resolvable on `PATH`.
    pub fn which(program: &str) -> bool {
        let Some(path) = std::env::var_os("PATH") else {
            return false;
        };
        std::env::split_paths(&path).any(|directory| {
            let candidate = directory.join(program);
            candidate.is_file()
        })
    }
}

// strictrs: capability
pub mod net {
    use std::io;
    use std::net::SocketAddr;

    /// Bind a blocking HTTP server. Callers choose the address; the CLI
    /// defaults it to loopback.
    pub fn bind(address: SocketAddr) -> io::Result<tiny_http::Server> {
        tiny_http::Server::http(address).map_err(io::Error::other)
    }

    /// POST a JSON body and return the response body as text.
    pub fn post_json(
        url: &str,
        bearer_token: &str,
        body: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let response = ureq::post(url)
            .header("Authorization", &format!("Bearer {bearer_token}"))
            .header("Content-Type", "application/json")
            .send(body)?;
        Ok(response.into_body().read_to_string()?)
    }

    /// POST a multipart body, used by the speech-to-text endpoint.
    pub fn post_multipart(
        url: &str,
        bearer_token: &str,
        boundary: &str,
        body: Vec<u8>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let response = ureq::post(url)
            .header("Authorization", &format!("Bearer {bearer_token}"))
            .header(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .send(&body[..])?;
        Ok(response.into_body().read_to_string()?)
    }

    /// POST JSON and return the raw response bytes, used by text-to-speech.
    pub fn post_json_bytes(
        url: &str,
        bearer_token: &str,
        body: &str,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let response = ureq::post(url)
            .header("Authorization", &format!("Bearer {bearer_token}"))
            .header("Content-Type", "application/json")
            .send(body)?;
        let mut reader = response.into_body().into_reader();
        let mut bytes = Vec::new();
        io::copy(&mut reader, &mut bytes)?;
        Ok(bytes)
    }
}
