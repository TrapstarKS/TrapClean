fn main() -> Result<(), Box<dyn std::error::Error>> {
    let identity = uuid::Uuid::new_v4().to_string();
    let entry = keyring::Entry::new("io.github.trapstarks.trapclean.validation", &identity)?;
    let value = "synthetic-local-vault-test-not-a-provider-credential";
    entry.set_password(value)?;
    let read = entry.get_password();
    let removed = entry.delete_credential();
    assert_eq!(read?, value);
    removed?;
    assert!(matches!(entry.get_password(), Err(keyring::Error::NoEntry)));
    println!("Native credential vault write/read/delete verified with a unique synthetic entry.");
    Ok(())
}
