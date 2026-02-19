use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

/// A test git repo with some commits across branches.
pub struct TestRepo {
    pub dir: TempDir,
}

impl TestRepo {
    /// Create a temp git repo with:
    /// - 3 commits on `main` branch by "Alice <alice@test.com>"
    /// - 1 commit on `feature` branch by "Bob <bob@test.com>"
    /// - Files: README.md, src/main.rs, src/lib.rs (added in different commits)
    pub fn new() -> Self {
        let dir = TempDir::new().expect("create temp dir");
        let path = dir.path();

        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(path)
                .env("GIT_AUTHOR_NAME", "Alice")
                .env("GIT_AUTHOR_EMAIL", "alice@test.com")
                .env("GIT_COMMITTER_NAME", "Alice")
                .env("GIT_COMMITTER_EMAIL", "alice@test.com")
                .output()
                .expect("git command failed");
            assert!(
                output.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
            output
        };

        // Init repo (use checkout -b for older git versions without init -b)
        git(&["init"]);
        git(&["checkout", "-b", "main"]);

        // Commit 1: add README
        std::fs::write(path.join("README.md"), "# Test\n").unwrap();
        git(&["add", "README.md"]);
        git(&["commit", "-m", "Initial commit: add README"]);

        // Commit 2: add src/main.rs
        std::fs::create_dir_all(path.join("src")).unwrap();
        std::fs::write(path.join("src/main.rs"), "fn main() {}\n").unwrap();
        git(&["add", "src/main.rs"]);
        git(&["commit", "-m", "Add main.rs entry point"]);

        // Commit 3: add src/lib.rs
        std::fs::write(path.join("src/lib.rs"), "pub fn hello() {}\n").unwrap();
        git(&["add", "src/lib.rs"]);
        git(&["commit", "-m", "Add lib.rs with hello function"]);

        // Create feature branch with a commit by Bob
        git(&["checkout", "-b", "feature"]);
        std::fs::write(path.join("src/lib.rs"), "pub fn hello() { println!(\"hello\"); }\n").unwrap();

        let output = Command::new("git")
            .args(["commit", "-am", "Update hello to print"])
            .current_dir(path)
            .env("GIT_AUTHOR_NAME", "Bob")
            .env("GIT_AUTHOR_EMAIL", "bob@test.com")
            .env("GIT_COMMITTER_NAME", "Bob")
            .env("GIT_COMMITTER_EMAIL", "bob@test.com")
            .output()
            .expect("git command failed");
        assert!(output.status.success(), "bob commit failed: {}", String::from_utf8_lossy(&output.stderr));

        // Switch back to main
        git(&["checkout", "main"]);

        TestRepo { dir }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }

    pub fn name(&self) -> String {
        self.dir
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string()
    }
}
