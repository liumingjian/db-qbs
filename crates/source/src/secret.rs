//! 数据源口令的静态加密（ADR-0037 §3）。
//!
//! **这一层防的是什么、不防什么，ADR-0037 §3 写死了，这里复述要点免得被高估：**
//! 防的是 SQLite 库文件**离开本机**之后的裸读（备份、快照、误发一份出去）；
//! **不防**拿到 `data_dir` 读权限的人——密钥就在同一个目录、同样 0600，
//! 对这类攻击者它只是多一步 `xor`。也**不防** `listen` 端口本身：
//! 能连上 source 的人本来就能发起运行（ADR-0024 §1 的权限等价一字不变）。

use std::fs::{self, OpenOptions, Permissions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rand::RngCore;

const KEY_FILE: &str = "datasource.key";
const KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 12;

/// 数据源口令的加解密器。密钥来自 `data_dir/datasource.key`，首次使用时生成。
pub struct SecretBox {
    cipher: ChaCha20Poly1305,
}

impl SecretBox {
    pub fn open(data_dir: &Path) -> Result<Self, String> {
        let key = load_or_create_key(data_dir)?;
        Ok(Self {
            cipher: ChaCha20Poly1305::new(Key::from_slice(&key)),
        })
    }

    /// 密文形态：`hex(nonce) || hex(ciphertext)`，一列存下。
    ///
    /// **每次加密取一个新的随机 nonce**——ChaCha20-Poly1305 在同一密钥下重复 nonce
    /// 会同时丢掉机密性与完整性，而「改个口令再存一次」正是会反复触发它的操作。
    pub fn seal(&self, plaintext: &str) -> Result<String, String> {
        let mut nonce_bytes = [0_u8; NONCE_BYTES];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let ciphertext = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce_bytes), plaintext.as_bytes())
            .map_err(|_| "加密数据源口令失败".to_owned())?;
        Ok(format!("{}{}", to_hex(&nonce_bytes), to_hex(&ciphertext)))
    }

    pub fn open_secret(&self, sealed: &str) -> Result<String, String> {
        let raw = from_hex(sealed).ok_or_else(|| "数据源口令密文不是有效的十六进制".to_owned())?;
        if raw.len() <= NONCE_BYTES {
            return Err("数据源口令密文过短".to_owned());
        }
        let (nonce_bytes, ciphertext) = raw.split_at(NONCE_BYTES);
        let plaintext = self
            .cipher
            .decrypt(Nonce::from_slice(nonce_bytes), ciphertext)
            // 解不开只有两种成因：换过密钥文件，或密文被改过。两者都不是「口令错了」，
            // 措辞必须把人引到密钥文件上，否则会去数据库那头白查一轮。
            .map_err(|_| {
                format!(
                    "解密数据源口令失败：密文与 {KEY_FILE} 对不上（密钥文件被换过或密文被改过）"
                )
            })?;
        String::from_utf8(plaintext).map_err(|_| "数据源口令密文解出的不是 UTF-8".to_owned())
    }
}

fn load_or_create_key(data_dir: &Path) -> Result<[u8; KEY_BYTES], String> {
    fs::create_dir_all(data_dir).map_err(|error| format!("创建 source 数据目录失败：{error}"))?;
    let path = data_dir.join(KEY_FILE);
    match OpenOptions::new().read(true).open(&path) {
        Ok(mut file) => {
            let mut key = [0_u8; KEY_BYTES];
            file.read_exact(&mut key)
                .map_err(|error| format!("读取数据源密钥文件 {} 失败：{error}", path.display()))?;
            Ok(key)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => create_key(&path),
        Err(error) => Err(format!(
            "打开数据源密钥文件 {} 失败：{error}",
            path.display()
        )),
    }
}

fn create_key(path: &Path) -> Result<[u8; KEY_BYTES], String> {
    let mut key = [0_u8; KEY_BYTES];
    rand::thread_rng().fill_bytes(&mut key);
    // `create_new` 而不是 `create`：两个进程同时首启时，输的那个必须失败，
    // 不能把已经用来加密过的密钥覆盖掉——那会让库里已有的密文永久解不开。
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("创建数据源密钥文件 {} 失败：{error}", path.display()))?;
    file.write_all(&key)
        .map_err(|error| format!("写入数据源密钥文件失败：{error}"))?;
    fs::set_permissions(path, Permissions::from_mode(0o600))
        .map_err(|error| format!("设置数据源密钥文件权限失败：{error}"))?;
    Ok(key)
}

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn from_hex(encoded: &str) -> Option<Vec<u8>> {
    if encoded.len() % 2 != 0 {
        return None;
    }
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks(2) {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        decoded.push((high * 16 + low) as u8);
    }
    Some(decoded)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn a_sealed_secret_round_trips_through_the_same_key_file() {
        let directory = temp_directory();
        let secrets = SecretBox::open(&directory).unwrap();
        let sealed = secrets.seal("change-me").unwrap();

        assert!(!sealed.contains("change-me"));
        assert_eq!(secrets.open_secret(&sealed).unwrap(), "change-me");
        // 重开一次（模拟重启）：密钥从文件读回来，旧密文照样解得开。
        let reopened = SecretBox::open(&directory).unwrap();
        assert_eq!(reopened.open_secret(&sealed).unwrap(), "change-me");

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn the_same_plaintext_seals_differently_every_time() {
        let directory = temp_directory();
        let secrets = SecretBox::open(&directory).unwrap();

        // nonce 每次新取——两次密文相同就说明 nonce 被复用了，那是这套加密的致命失效。
        assert_ne!(
            secrets.seal("change-me").unwrap(),
            secrets.seal("change-me").unwrap()
        );

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_secret_sealed_under_another_key_names_the_key_file() {
        let one = temp_directory();
        let other = temp_directory();
        let sealed = SecretBox::open(&one).unwrap().seal("change-me").unwrap();

        let error = SecretBox::open(&other)
            .unwrap()
            .open_secret(&sealed)
            .unwrap_err();
        assert!(error.contains(KEY_FILE), "{error}");

        std::fs::remove_dir_all(one).unwrap();
        std::fs::remove_dir_all(other).unwrap();
    }

    fn temp_directory() -> PathBuf {
        let suffix = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "db-qbs-source-secret-test-{}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        path
    }
}
