use std::process::Command;

#[test]
fn s4_probe() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/s4probe");
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "kp"))
        .collect();
    files.sort();
    for p in files {
        let stem = p.file_stem().unwrap().to_string_lossy().to_string();
        let out = std::env::temp_dir().join(format!("s4probe-{stem}"));
        let o = Command::new(env!("CARGO_BIN_EXE_kelpc")).arg("build").arg(&p).arg("-o").arg(&out).output().unwrap();
        println!("PROBE {stem} status={:?}\n{}", o.status.code(), String::from_utf8_lossy(&o.stderr));
        if o.status.success() {
            let r = Command::new(&out).output().unwrap();
            println!("RUN {stem} stdout={:?} status={:?}", String::from_utf8_lossy(&r.stdout), r.status.code());
        }
    }
}
