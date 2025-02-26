use std::io::Result;

fn main() -> Result<()> {
    let mut protos = Vec::new();
    if let Ok(entries) = std::fs::read_dir("./src") {
        for entry in entries {
            match entry {
                Ok(entry) => {
                    if entry.path().extension().is_some_and(|ext| ext == "proto") {
                        println!("Found {:?}", entry.path());
                        protos.push(entry.path())
                    }
                }
                Err(_) => continue,
            }
        }
    }
    println!("Building: {:?}", protos);

    prost_build::compile_protos(&protos, &["src/"])?;

    Ok(())
}
