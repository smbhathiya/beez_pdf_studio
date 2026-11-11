//! Document info & metadata, text extraction, password protection.

use std::collections::BTreeMap;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context, Result, anyhow, bail};
use lopdf::encryption::crypt_filters::{Aes128CryptFilter, CryptFilter};
use lopdf::{Document, EncryptionState, EncryptionVersion, Object, Permissions, StringFormat};

use super::{
    decode_text_string, encode_text_string, expand_ranges, human_size, load, page_box,
    parse_ranges, save,
};

pub const FIELDS: [&str; 6] = [
    "Title", "Author", "Subject", "Keywords", "Creator", "Producer",
];

#[derive(Clone, Debug, Default)]
pub struct DocInfo {
    pub pages: u32,
    pub version: String,
    pub file_size: String,
    pub first_page_size: String,
    pub was_encrypted: bool,
    pub objects: usize,
    /// Values for [`FIELDS`], in order.
    pub fields: Vec<String>,
}

pub fn read_info(path: &Path) -> Result<DocInfo> {
    let doc = load(path)?;
    let info = doc
        .trailer
        .get(b"Info")
        .ok()
        .and_then(|o| doc.dereference(o).ok())
        .and_then(|(_, o)| o.as_dict().ok().cloned())
        .unwrap_or_default();
    let fields = FIELDS
        .iter()
        .map(|f| {
            match info
                .get(f.as_bytes())
                .ok()
                .and_then(|o| doc.dereference(o).ok())
            {
                Some((_, Object::String(s, _))) => decode_text_string(s),
                _ => String::new(),
            }
        })
        .collect();
    let first_page_size = doc
        .get_pages()
        .get(&1)
        .map(|&id| {
            let [x0, y0, x1, y1] = page_box(&doc, id);
            let (w, h) = (x1 - x0, y1 - y0);
            format!(
                "{:.0} × {:.0} pt ({:.0} × {:.0} mm)",
                w,
                h,
                w / 72.0 * 25.4,
                h / 72.0 * 25.4
            )
        })
        .unwrap_or_default();
    Ok(DocInfo {
        pages: doc.get_pages().len() as u32,
        version: doc.version.clone(),
        file_size: human_size(fs::metadata(path).map(|m| m.len()).unwrap_or(0)),
        first_page_size,
        was_encrypted: doc.encryption_state.is_some(),
        objects: doc.objects.len(),
        fields,
    })
}

fn pdf_date_now() -> String {
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    // Civil date from days since epoch (Howard Hinnant's algorithm), UTC.
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "D:{y:04}{m:02}{d:02}{:02}{:02}{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// `fields` holds values for [`FIELDS`]; empty strings remove the entry.
pub fn write_info(input: &Path, output: &Path, fields: &[String]) -> Result<String> {
    let mut doc = load(input)?;
    let mut info = doc
        .trailer
        .get(b"Info")
        .ok()
        .and_then(|o| doc.dereference(o).ok())
        .and_then(|(_, o)| o.as_dict().ok().cloned())
        .unwrap_or_default();
    for (key, value) in FIELDS.iter().zip(fields) {
        if value.trim().is_empty() {
            info.remove(key.as_bytes());
        } else {
            info.set(*key, encode_text_string(value.trim()));
        }
    }
    info.set("ModDate", Object::string_literal(pdf_date_now()));
    if !info.has(b"CreationDate") {
        info.set("CreationDate", Object::string_literal(pdf_date_now()));
    }
    // XMP metadata would contradict the new values; readers prefer XMP, so drop it.
    if let Ok(cat) = doc.catalog_mut() {
        cat.remove(b"Metadata");
    }
    let id = doc.add_object(info);
    doc.trailer.set("Info", id);
    save(&mut doc, output)?;
    Ok(format!("Metadata saved → {}", output.display()))
}

pub fn extract_text(input: &Path, output: &Path, pages: &str) -> Result<String> {
    let doc = load(input)?;
    let list = expand_ranges(&parse_ranges(pages, super::page_count(&doc))?);
    let mut text = String::new();
    for p in &list {
        text.push_str(&format!("===== Page {p} =====\n"));
        text.push_str(
            &doc.extract_text(&[*p])
                .unwrap_or_else(|e| format!("[could not extract: {e}]\n")),
        );
        text.push('\n');
    }
    fs::write(output, &text).with_context(|| format!("Could not write {}", output.display()))?;
    Ok(format!(
        "Extracted {} characters from {} page(s) → {}",
        text.chars().count(),
        list.len(),
        output.display()
    ))
}

#[derive(Clone, Copy, Debug)]
pub struct Allow {
    pub print: bool,
    pub copy: bool,
    pub modify: bool,
    pub annotate: bool,
    pub fill_forms: bool,
    pub assemble: bool,
}

impl Default for Allow {
    fn default() -> Self {
        Self {
            print: true,
            copy: false,
            modify: false,
            annotate: false,
            fill_forms: true,
            assemble: false,
        }
    }
}

/// Protect with AES-128. `user_pw` is needed to open; `owner_pw` lifts restrictions.
pub fn protect(
    input: &Path,
    output: &Path,
    user_pw: &str,
    owner_pw: &str,
    allow: Allow,
) -> Result<String> {
    if user_pw.is_empty() && owner_pw.is_empty() {
        bail!("Enter at least one password");
    }
    let owner_pw = if owner_pw.is_empty() {
        user_pw
    } else {
        owner_pw
    };
    let mut doc = load(input)?;
    doc.encryption_state = None;

    if !doc.trailer.has(b"ID") {
        let mut h = DefaultHasher::new();
        input.hash(&mut h);
        SystemTime::now().hash(&mut h);
        let a = h.finish().to_be_bytes();
        doc.objects.len().hash(&mut h);
        let b = h.finish().to_be_bytes();
        let id: Vec<u8> = a.iter().chain(b.iter()).copied().collect();
        doc.trailer.set(
            "ID",
            vec![
                Object::String(id.clone(), StringFormat::Hexadecimal),
                Object::String(id, StringFormat::Hexadecimal),
            ],
        );
    }

    let mut perms = Permissions::COPYABLE_FOR_ACCESSIBILITY;
    if allow.print {
        perms |= Permissions::PRINTABLE | Permissions::PRINTABLE_IN_HIGH_QUALITY;
    }
    if allow.copy {
        perms |= Permissions::COPYABLE;
    }
    if allow.modify {
        perms |= Permissions::MODIFIABLE;
    }
    if allow.annotate {
        perms |= Permissions::ANNOTABLE;
    }
    if allow.fill_forms {
        perms |= Permissions::FILLABLE;
    }
    if allow.assemble {
        perms |= Permissions::ASSEMBLABLE;
    }

    let filter: Arc<dyn CryptFilter> = Arc::new(Aes128CryptFilter);
    let version = EncryptionVersion::V4 {
        document: &doc,
        encrypt_metadata: true,
        crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), filter)]),
        stream_filter: b"StdCF".to_vec(),
        string_filter: b"StdCF".to_vec(),
        owner_password: owner_pw,
        user_password: user_pw,
        permissions: perms,
    };
    let state =
        EncryptionState::try_from(version).map_err(|e| anyhow!("Encryption failed: {e:?}"))?;
    doc.encrypt(&state)
        .map_err(|e| anyhow!("Encryption failed: {e}"))?;
    save(&mut doc, output)?;
    Ok(format!("Protected with AES-128 → {}", output.display()))
}

pub fn unlock(input: &Path, output: &Path, password: &str) -> Result<String> {
    let mut doc = Document::load_with_password(input, password)
        .map_err(|e| anyhow!("Could not open {} – wrong password? ({e})", input.display()))?;
    if doc.encryption_state.is_none() && !doc.is_encrypted() {
        bail!("This PDF is not password protected");
    }
    if doc.is_encrypted() {
        doc.decrypt(password)
            .map_err(|e| anyhow!("Wrong password ({e})"))?;
    }
    doc.encryption_state = None;
    save(&mut doc, output)?;
    Ok(format!("Password removed → {}", output.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_text_and_passwords() {
        let dir = std::env::temp_dir().join("utilbee_meta_test");
        fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.pdf");
        save(&mut crate::pdf::tests::sample(2), &a).unwrap();

        let b = dir.join("b.pdf");
        let mut fields = vec![String::new(); 6];
        fields[0] = "Hello Wörld".into();
        fields[1] = "UtilBee".into();
        write_info(&a, &b, &fields).unwrap();
        let info = read_info(&b).unwrap();
        assert_eq!(info.fields[0], "Hello Wörld");
        assert_eq!(info.fields[1], "UtilBee");
        assert_eq!(info.pages, 2);

        let t = dir.join("t.txt");
        extract_text(&b, &t, "2").unwrap();
        assert!(fs::read_to_string(&t).unwrap().contains("Page 2"));

        let locked = dir.join("locked.pdf");
        protect(&b, &locked, "secret", "owner", Allow::default()).unwrap();
        assert!(load(&locked).is_err(), "should need a password");
        let open = dir.join("open.pdf");
        assert!(unlock(&locked, &open, "nope").is_err());
        unlock(&locked, &open, "secret").unwrap();
        let back = Document::load(&open).unwrap();
        assert!(back.encryption_state.is_none());
        assert!(back.extract_text(&[1]).unwrap().contains("Page 1"));
    }
}
