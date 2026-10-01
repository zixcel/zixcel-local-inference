use std::fs;
use std::path::Path;
use std::process::ExitCode;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signer, SigningKey};
use rand_core::OsRng;
use zixcel_local_inference::{Catalog, SIGNED_CATALOG_SCHEMA, SignedCatalog};

fn main() -> ExitCode {
    match execute() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::from(2)
        }
    }
}

fn execute() -> Result<(), &'static str> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let [input, output] = arguments.as_slice() else {
        return Err("usage: sign_catalog INPUT_CATALOG_JSON OUTPUT_DIRECTORY");
    };
    let payload = fs::read(input).map_err(|_| "catalog-read-failed")?;
    let catalog: Catalog =
        serde_json::from_slice(&payload).map_err(|_| "catalog-payload-invalid")?;
    let output = Path::new(output);
    fs::create_dir_all(output).map_err(|_| "output-create-failed")?;
    let signing_key = SigningKey::generate(&mut OsRng);
    let key_id = format!("demo-{}-sequence-{}", catalog.catalog_id, catalog.sequence);
    let envelope = SignedCatalog {
        schema: SIGNED_CATALOG_SCHEMA.to_owned(),
        key_id,
        payload_base64: STANDARD.encode(&payload),
        signature_base64: STANDARD.encode(signing_key.sign(&payload).to_bytes()),
    };
    fs::write(
        output.join("catalog.signed.json"),
        serde_json::to_vec_pretty(&envelope).map_err(|_| "catalog-envelope-invalid")?,
    )
    .map_err(|_| "catalog-write-failed")?;
    fs::write(
        output.join("catalog.public-key.txt"),
        STANDARD.encode(signing_key.verifying_key().to_bytes()),
    )
    .map_err(|_| "public-key-write-failed")?;
    println!("key-id={}", envelope.key_id);
    println!("catalog={}", output.join("catalog.signed.json").display());
    println!(
        "public-key={}",
        output.join("catalog.public-key.txt").display()
    );
    Ok(())
}
