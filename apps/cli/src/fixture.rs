//! Synthetic test trees: plain, NFC-accented and NFD-accented names, deterministic content.

use std::path::PathBuf;

use anyhow::{Result, bail};

#[derive(clap::Args)]
pub struct Args {
    /// Folder to create (must be missing or empty).
    dir: PathBuf,
    #[arg(long, default_value_t = 1000)]
    files: usize,
    /// Bytes per file.
    #[arg(long, default_value_t = 64 * 1024)]
    size: usize,
    #[arg(long, default_value_t = 20)]
    dirs: usize,
    /// Share of accented names written in NFD.
    #[arg(long, default_value_t = 30)]
    nfd_percent: usize,
}

/// (NFC, NFD) spellings.
const WORDS: [(&str, &str); 9] = [
    ("Fotografias", "Fotografias"),
    ("Ação", "Ac\u{327}a\u{303}o"),
    ("Verão", "Vera\u{303}o"),
    ("Relatório", "Relato\u{301}rio"),
    ("Orçamento", "Orc\u{327}amento"),
    ("Café", "Cafe\u{301}"),
    ("Índice", "I\u{301}ndice"),
    ("Médico", "Me\u{301}dico"),
    ("Coração", "Corac\u{327}a\u{303}o"),
];

fn word(i: usize, salt: usize, nfd_percent: usize) -> &'static str {
    let (nfc, nfd) = WORDS[i % WORDS.len()];
    if (i * salt) % 100 < nfd_percent {
        nfd
    } else {
        nfc
    }
}

pub fn run(args: &Args) -> Result<()> {
    if args.dir.exists() && std::fs::read_dir(&args.dir)?.next().is_some() {
        bail!("{} is not empty", args.dir.display());
    }
    let dirs: Vec<PathBuf> = (0..args.dirs.max(1))
        .map(|j| {
            args.dir
                .join(format!("{}-{j:03}", word(j, 53, args.nfd_percent)))
        })
        .collect();
    for d in &dirs {
        std::fs::create_dir_all(d)?;
    }
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut buf = vec![0u8; args.size];
    for i in 0..args.files {
        for b in buf.iter_mut() {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *b = (state >> 56) as u8;
        }
        let name = format!("{}-{i:06}.bin", word(i, 37, args.nfd_percent));
        std::fs::write(dirs[i % dirs.len()].join(name), &buf)?;
    }
    println!(
        "created {} files ({:.1} MB) in {} dirs under {}",
        args.files,
        (args.files * args.size) as f64 / 1e6,
        dirs.len(),
        args.dir.display()
    );
    Ok(())
}
