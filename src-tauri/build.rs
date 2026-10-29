fn main() {
    println!("cargo:rerun-if-changed=../src");
    println!("cargo:rerun-if-changed=icons");
    println!("cargo:rerun-if-changed=hardware/Program.cs");
    println!("cargo:rerun-if-changed=hardware/Service.cs");
    println!("cargo:rerun-if-changed=hardware/build.ps1");
    println!("cargo:rerun-if-changed=hardware/vendor");
    println!("cargo:rerun-if-changed=hardware/licenses");
    #[cfg(windows)] {
        let status=std::process::Command::new("powershell.exe").args(["-NoProfile","-File","hardware/build.ps1"])
            .status().expect("could not run sensor helper compiler");
        assert!(status.success(),"sensor helper compilation failed");
        let out=std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
        let destination=out.ancestors().nth(3).unwrap().join("hardware");
        std::fs::create_dir_all(&destination).unwrap();
        for entry in std::fs::read_dir("hardware/runtime").unwrap().flatten() {
            if entry.path().is_file() {
                let target=destination.join(entry.file_name());
                if std::fs::read(entry.path()).ok()!=std::fs::read(&target).ok() {
                    std::fs::copy(entry.path(),target).unwrap();
                }
            }
        }
    }
    tauri_build::build()
}
