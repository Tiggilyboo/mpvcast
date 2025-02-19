use std::io::Result;

fn main() -> Result<()> {
    let mut protos = Vec::new();
    if let Ok(entries) = std::fs::read_dir("./src/proto") {
        for entry in entries {
            match entry {
                Ok(entry) => {
                    println!("Found {:?}", entry.path());
                    if entry.path().extension().is_some_and(|ext| ext == "proto") {
                        protos.push(entry.path())
                    }
                }
                Err(_) => continue,
            }
        }
    }
    prost_build::compile_protos(&protos, &["src/"])?;

    Ok(())
}
