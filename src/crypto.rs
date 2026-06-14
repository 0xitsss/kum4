use cipher::generic_array::GenericArray;
use cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use rand::RngCore;
use whirlpool::{Digest, Whirlpool};

use crate::error::{Kum4Error, Result};

const SALT_LEN: usize = 16;
const IV_LEN: usize = 16;
const BLOCK_SIZE: usize = 16;

pub fn encrypt(plaintext: &[u8], password: &str) -> Vec<u8> {
    let mut salt = [0u8; SALT_LEN];
    let mut iv = [0u8; IV_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    rand::thread_rng().fill_bytes(&mut iv);

    let key = derive_key_32(password, &salt);
    let cipher = twofish::Twofish::new(GenericArray::from_slice(&key));

    let pad_len = BLOCK_SIZE - (plaintext.len() % BLOCK_SIZE);
    let total = plaintext.len() + pad_len;
    let mut padded = Vec::with_capacity(total);
    padded.extend_from_slice(plaintext);
    padded.resize(total, pad_len as u8);

    let mut out = vec![0u8; total];
    let mut prev = iv;
    for (chunk, slot) in padded.chunks(BLOCK_SIZE).zip(out.chunks_mut(BLOCK_SIZE)) {
        let mut block = GenericArray::clone_from_slice(&prev);
        for (b, c) in block.iter_mut().zip(chunk) {
            *b ^= c;
        }
        cipher.encrypt_block(&mut block);
        slot.copy_from_slice(&block);
        prev.copy_from_slice(&block);
    }

    let mut result = Vec::with_capacity(SALT_LEN + IV_LEN + out.len());
    result.extend_from_slice(&salt);
    result.extend_from_slice(&iv);
    result.extend_from_slice(&out);
    result
}

fn derive_key_32(password: &str, salt: &[u8]) -> [u8; 32] {
    let mut hasher = Whirlpool::new();
    hasher.update(salt);
    hasher.update(password.as_bytes());
    let hash = hasher.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&hash[..32]);
    key
}

fn derive_key_16(password: &str, salt: &[u8]) -> [u8; 16] {
    let mut hasher = Whirlpool::new();
    hasher.update(salt);
    hasher.update(password.as_bytes());
    let hash = hasher.finalize();
    let mut key = [0u8; 16];
    key.copy_from_slice(&hash[..16]);
    key
}

fn cbc_decrypt<C: BlockDecrypt>(ciphertext: &[u8], iv: &[u8], cipher: &C) -> Option<Vec<u8>> {
    let mut plain = vec![0u8; ciphertext.len()];
    let mut prev = iv.to_vec();
    for (chunk, slot) in ciphertext
        .chunks(BLOCK_SIZE)
        .zip(plain.chunks_mut(BLOCK_SIZE))
    {
        let mut block = GenericArray::clone_from_slice(chunk);
        cipher.decrypt_block(&mut block);
        for i in 0..BLOCK_SIZE {
            slot[i] = block[i] ^ prev[i];
        }
        prev.copy_from_slice(chunk);
    }

    let pad_byte = *plain.last()?;
    if pad_byte == 0 || pad_byte > BLOCK_SIZE as u8 {
        return None;
    }
    let pad_len = pad_byte as usize;
    if plain.len() < pad_len {
        return None;
    }
    if plain[plain.len() - pad_len..]
        .iter()
        .any(|&b| b != pad_byte)
    {
        return None;
    }
    plain.truncate(plain.len() - pad_len);
    Some(plain)
}

pub fn decrypt(data: &[u8], password: &str) -> Option<Vec<u8>> {
    if data.len() < SALT_LEN + IV_LEN + BLOCK_SIZE {
        return None;
    }
    let salt = &data[..SALT_LEN];
    let iv = &data[SALT_LEN..SALT_LEN + IV_LEN];
    let ciphertext = &data[SALT_LEN + IV_LEN..];
    if !ciphertext.len().is_multiple_of(BLOCK_SIZE) {
        return None;
    }

    let key32 = derive_key_32(password, salt);
    let cipher32 = twofish::Twofish::new(GenericArray::from_slice(&key32));
    if let Some(plain) = cbc_decrypt(ciphertext, iv, &cipher32) {
        return Some(plain);
    }

    let key16 = derive_key_16(password, salt);
    let cipher16 = twofish::Twofish::new(GenericArray::from_slice(&key16));
    if let Some(plain) = cbc_decrypt(ciphertext, iv, &cipher16) {
        return Some(plain);
    }

    None
}

pub fn load_or_generate_key(key_path: &str) -> Result<String> {
    if std::path::Path::new(key_path).exists() {
        let data = std::fs::read(key_path)
            .map_err(|e| Kum4Error::Config(format!("Failed to read {key_path}: {e}")))?;
        for attempt in 0..3 {
            let password = prompt_password(&format!(
                "Enter wallet password (attempt {}/3): ",
                attempt + 1
            ))?;
            if let Some(plain) = decrypt(&data, &password) {
                match String::from_utf8(plain) {
                    Ok(s) => return Ok(s),
                    Err(e) => return Err(Kum4Error::Config(format!("Invalid key data: {e}"))),
                }
            }
            if attempt < 2 {
                println!("Wrong password. Try again.");
            }
        }
        Err(Kum4Error::Config(
            "Invalid password after 3 attempts. Delete key.kum4 and restart to generate a new wallet."
                .into(),
        ))
    } else {
        let mut entropy = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut entropy);
        let mnemonic = bip39::Mnemonic::from_entropy(&entropy)
            .map_err(|e| Kum4Error::Wallet(format!("Mnemonic generation: {e}")))?;
        let phrase = mnemonic.to_string();

        println!("\n===== NEW WALLET GENERATED =====");
        println!("Seed phrase (SAVE THIS SAFELY):");
        println!("\n  {}\n", &phrase);
        println!("This is the ONLY way to recover your wallet.");
        println!("=================================\n");

        let password = loop {
            let p1 = prompt_password("Set wallet password: ")?;
            let p2 = prompt_password("Confirm password: ")?;
            if p1 == p2 {
                break p1;
            }
            println!("Passwords do not match, try again.");
        };

        let ciphertext = encrypt(phrase.as_bytes(), &password);
        std::fs::write(key_path, &ciphertext)
            .map_err(|e| Kum4Error::Config(format!("Failed to write {key_path}: {e}")))?;
        println!("Encrypted key saved to {key_path}");

        println!("\nPress Enter to confirm you have saved the seed phrase...");
        let mut confirm = String::new();
        std::io::stdin().read_line(&mut confirm).ok();

        Ok(phrase)
    }
}

fn prompt_password(prompt: &str) -> Result<String> {
    use std::io::Write;
    print!("{prompt}");
    std::io::stdout().flush().ok();
    rpassword::read_password().map_err(|e| Kum4Error::Config(format!("Password read error: {e}")))
}
