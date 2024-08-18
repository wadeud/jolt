use std::{
    fs::{self, File},
    future::Future,
    io::Write,
    path::PathBuf,
};

use dirs::home_dir;
use eyre::{bail, eyre, Result};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
#[cfg(not(target_arch = "wasm32"))]
use tokio::runtime::Runtime;

const TOOLCHAIN_TAG: &str = include_str!("../../../.jolt.rust.toolchain-tag");
const DOWNLOAD_RETRIES: usize = 5;
const DELAY_BASE_MS: u64 = 500;

#[cfg(not(target_arch = "wasm32"))]
/// Installs the toolchain if it is not already present.
pub fn install_toolchain() -> Result<()> {
    if !has_toolchain() {
        let client = Client::builder().user_agent("Mozilla/5.0").build()?;
        let toolchain_url = toolchain_url();

        let rt = Runtime::new()?;
        rt.block_on(retry_times(DOWNLOAD_RETRIES, DELAY_BASE_MS, || {
            download_toolchain(&client, &toolchain_url)
        }))?;
        unpack_toolchain()?;
        write_tag_file()?;
    }
    link_toolchain()
}

#[cfg(not(target_arch = "wasm32"))]
/// Retries a given asynchronous function with exponential backoff.
async fn retry_times<F, T, E>(times: usize, base_ms: u64, f: F) -> Result<T>
where
    F: Fn() -> E,
    E: Future<Output = Result<T>>,
{
    for i in 0..times {
        println!("Attempt {}/{}", i + 1, times);
        match f().await {
            Ok(t) => return Ok(t),
            Err(e) => {
                let timeout = delay_timeout(i, base_ms);
                println!("Error on attempt {}/{}: {}. Retrying in {}ms", i + 1, times, e, timeout);
                tokio::time::sleep(std::time::Duration::from_millis(timeout)).await;
            }
        }
    }
    Err(eyre!("Failed after {} retries", times))
}

/// Calculates exponential backoff delay.
fn delay_timeout(i: usize, base_ms: u64) -> u64 {
    let timeout = 2u64.pow(i as u32) * base_ms;
    rand::random::<u64>() % timeout
}

/// Writes the toolchain tag to a file.
fn write_tag_file() -> Result<()> {
    let tag_path = toolchain_tag_file();
    let mut tag_file = File::create(tag_path)?;
    tag_file.write_all(TOOLCHAIN_TAG.as_bytes())?;
    Ok(())
}

/// Links the toolchain using `rustup`.
fn link_toolchain() -> Result<()> {
    let link_path = jolt_dir().join("rust/build/host/stage2");
    let output = std::process::Command::new("rustup")
        .args([
            "toolchain",
            "link",
            "riscv32i-jolt-zkvm-elf",
            link_path.to_str().unwrap(),
        ])
        .output()?;

    if !output.status.success() {
        bail!("{}", String::from_utf8(output.stderr)?);
    }

    Ok(())
}

/// Unpacks the downloaded toolchain archive.
fn unpack_toolchain() -> Result<()> {
    let output = std::process::Command::new("tar")
        .args(["-xzf", "rust-toolchain.tar.gz"])
        .current_dir(jolt_dir())
        .output()?;

    if !output.status.success() {
        bail!("{}", String::from_utf8(output.stderr)?);
    }

    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
/// Downloads the toolchain from the specified URL.
async fn download_toolchain(client: &Client, url: &str) -> Result<()> {
    let jolt_dir = jolt_dir();
    let output_path = jolt_dir.join("rust-toolchain.tar.gz");
    if !jolt_dir.exists() {
        fs::create_dir_all(&jolt_dir)?;
    }

    println!("Downloading toolchain from {}", url);
    let mut response = client.get(url).send().await?;
    if response.status().is_success() {
        let mut file = File::create(output_path)?;
        let total_size = response.content_length().unwrap_or(0);

        let pb = ProgressBar::new(total_size);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")?
                .progress_chars("#>-"),
        );

        let mut downloaded: u64 = 0;
        while let Some(chunk) = response.chunk().await? {
            file.write_all(&chunk)?;
            let new = downloaded + (chunk.len() as u64);
            pb.set_position(new);
            downloaded = new;
        }

        pb.finish_with_message("Download complete");

        Ok(())
    } else {
        Err(eyre!("Failed to download toolchain: {}", response.status()))
    }
}

/// Constructs the URL for downloading the toolchain.
fn toolchain_url() -> String {
    let target = target_lexicon::HOST;
    format!(
        "https://github.com/a16z/rust/releases/download/{}/rust-toolchain-{}.tar.gz",
        TOOLCHAIN_TAG, target,
    )
}

/// Checks if the toolchain is already installed by verifying the tag file.
fn has_toolchain() -> bool {
    let tag_path = toolchain_tag_file();
    if let Ok(tag) = fs::read_to_string(tag_path) {
        tag == TOOLCHAIN_TAG
    } else {
        false
    }
}

/// Returns the path to the Jolt directory in the user's home directory.
fn jolt_dir() -> PathBuf {
    home_dir().unwrap().join(".jolt")
}

/// Returns the path to the toolchain tag file.
fn toolchain_tag_file() -> PathBuf {
    jolt_dir().join(".toolchaintag")
}
