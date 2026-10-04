fn main() {
    println!("cargo:rerun-if-env-changed=IOS_RELEASE_DISTRIBUTION_VERSION");
    println!(
        "cargo:rustc-env=IOS_RELEASE_DISTRIBUTION_VERSION={}",
        std::env::var("IOS_RELEASE_DISTRIBUTION_VERSION").unwrap_or_default()
    );
    println!("cargo:rerun-if-env-changed=IOS_RELEASE_BUILD_REVISION");
    for name in ["HEAD", "refs/heads"] {
        if let Ok(output) = std::process::Command::new("git")
            .args(["rev-parse", "--git-path", name])
            .output()
            && output.status.success()
        {
            println!(
                "cargo:rerun-if-changed={}",
                String::from_utf8_lossy(&output.stdout).trim()
            );
        }
    }
    let revision = std::env::var("IOS_RELEASE_BUILD_REVISION")
        .ok()
        .or_else(|| {
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .output()
                .ok()
                .filter(|r| r.status.success())
                .map(|r| String::from_utf8_lossy(&r.stdout).trim().to_owned())
        })
        .unwrap_or_default();
    println!("cargo:rustc-env=IOS_RELEASE_BUILD_REVISION={revision}");
}
