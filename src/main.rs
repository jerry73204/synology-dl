use std::path::PathBuf;

use clap::Parser;
use eyre::{bail, Context, Result};
use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use regex::Regex;
use reqwest::Client;
use serde::Deserialize;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tracing::{info, warn};
use url::Url;

/// Synology Drive public link downloader
#[derive(Parser, Debug)]
#[command(name = "synology-dl")]
#[command(about = "Downloads files from Synology Drive public sharing links")]
struct Args {
    /// Synology Drive public sharing URL
    #[arg(value_name = "URL")]
    share_url: String,

    /// Output directory
    #[arg(default_value = ".")]
    output_dir: PathBuf,

    /// Show file info only, don't download
    #[arg(short, long)]
    info: bool,

    /// Force overwrite without prompting
    #[arg(short, long)]
    force: bool,
}

#[derive(thiserror::Error, Debug)]
enum AppError {
    #[error("Invalid Synology Drive share URL format. Expected: https://host/drive/d/s/PERMANENT_LINK/SHARING_LINK")]
    InvalidUrlFormat,

    #[error("Failed to get session cookie from share URL")]
    NoCookie,

    #[error("Could not find getDriveFile function in API response")]
    NoDriveFileFunction,

    #[error("No file_id in metadata")]
    NoFileId,
}

#[derive(Debug)]
struct ShareUrlParts {
    #[allow(dead_code)]
    host: String,
    base_url: String,
    permanent_link: String,
    sharing_link: String,
}

#[derive(Debug, Deserialize)]
struct FileMetadata {
    file_id: String,
    name: String,
    #[serde(default)]
    size: u64,
}

fn parse_share_url(url_str: &str) -> Result<ShareUrlParts> {
    let re = Regex::new(r"^https?://([^/]+)/drive/d/s/([^/]+)/([^/?]+)")?;

    let caps = re.captures(url_str).ok_or(AppError::InvalidUrlFormat)?;

    let host = caps.get(1).unwrap().as_str().to_string();
    let permanent_link = caps.get(2).unwrap().as_str().to_string();
    let sharing_link = caps.get(3).unwrap().as_str().to_string();
    let base_url = format!("https://{}", host);

    info!(host = %host, permanent_link = %permanent_link, sharing_link = %sharing_link, "Parsed share URL");

    Ok(ShareUrlParts {
        host,
        base_url,
        permanent_link,
        sharing_link,
    })
}

async fn get_session_cookie(client: &Client, share_url: &str) -> Result<(String, String)> {
    info!("Getting session cookie...");

    let resp = client
        .get(share_url)
        .send()
        .await
        .wrap_err("Failed to fetch share URL")?;

    for cookie in resp.cookies() {
        let name = cookie.name();
        if name.starts_with("drive-sharing-") {
            let value = cookie.value().to_string();
            let full_cookie = format!("{}={}", name, value);
            info!(cookie_len = value.len(), "Session cookie obtained");
            return Ok((full_cookie, value));
        }
    }

    Err(AppError::NoCookie.into())
}

fn extract_json_from_js(js_content: &str) -> Result<FileMetadata> {
    // Find getDriveFile function and extract the JSON object
    let start_marker = "getDriveFile=function(){return ";
    let alt_marker = "window.getDriveFile=function(){return ";

    let json_start = js_content
        .find(start_marker)
        .map(|pos| pos + start_marker.len())
        .or_else(|| {
            js_content
                .find(alt_marker)
                .map(|pos| pos + alt_marker.len())
        })
        .ok_or(AppError::NoDriveFileFunction)?;

    // Find matching closing brace by counting
    let mut depth = 0;
    let mut json_end = json_start;

    for (i, c) in js_content[json_start..].chars().enumerate() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    json_end = json_start + i + 1;
                    break;
                }
            }
            _ => {}
        }
    }

    let json_str = &js_content[json_start..json_end];
    let metadata: FileMetadata =
        serde_json::from_str(json_str).wrap_err("Failed to parse file metadata JSON")?;

    if metadata.file_id.is_empty() {
        return Err(AppError::NoFileId.into());
    }

    Ok(metadata)
}

async fn get_file_metadata(client: &Client, parts: &ShareUrlParts) -> Result<FileMetadata> {
    info!("Getting file metadata...");

    let perm_str = format!("\"{}\"", parts.permanent_link);
    let share_str = format!("\"{}\"", parts.sharing_link);
    let encoded_perm = urlencoding::encode(&perm_str);
    let encoded_share = urlencoding::encode(&share_str);

    let api_url = format!(
        "{}/drive/webapi/entry.cgi?api=SYNO.SynologyDrive.Shard&version=1&method=getjs&permanent_link={}&sharing_link={}",
        parts.base_url, encoded_perm, encoded_share
    );

    let resp = client
        .get(&api_url)
        .send()
        .await
        .wrap_err("Failed to fetch metadata API")?;

    let js_content = resp.text().await.wrap_err("Failed to read API response")?;
    let metadata = extract_json_from_js(&js_content)?;

    info!(
        file_name = %metadata.name,
        file_id = %metadata.file_id,
        size_bytes = metadata.size,
        "File metadata retrieved"
    );

    Ok(metadata)
}

fn format_size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;

    if bytes >= GIB {
        format!("{:.2} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.2} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.2} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}

async fn download_file(
    client: &Client,
    parts: &ShareUrlParts,
    metadata: &FileMetadata,
    cookie: &str,
    cookie_value: &str,
    output_dir: &PathBuf,
    force: bool,
) -> Result<()> {
    info!("Preparing download...");

    let token_str = format!("\"{}\"", cookie_value);
    let files_str = format!("[\"id:{}\"]", metadata.file_id);
    let encoded_token = urlencoding::encode(&token_str);
    let encoded_files = urlencoding::encode(&files_str);
    let encoded_filename = urlencoding::encode(&metadata.name);

    let download_url = format!(
        "{}/drive/d/s/{}/webapi/entry.cgi/SYNO.SynologyDrive.Files/{}?api=SYNO.SynologyDrive.Files&method=download&version=2&download_type=%22download%22&files={}&force_download=true&json_error=true&sharing_token={}",
        parts.base_url, parts.permanent_link, encoded_filename, encoded_files, encoded_token
    );

    // Create output directory if needed
    tokio::fs::create_dir_all(output_dir)
        .await
        .wrap_err("Failed to create output directory")?;

    let output_path = output_dir.join(&metadata.name);

    // Check if file already exists
    if output_path.exists() {
        if force {
            warn!("Overwriting existing file: {}", output_path.display());
        } else {
            warn!("File already exists: {}", output_path.display());
            print!("Overwrite? [y/N] ");
            use std::io::Write;
            std::io::stdout().flush()?;

            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;

            if !input.trim().eq_ignore_ascii_case("y") {
                info!("Download cancelled");
                return Ok(());
            }
        }
    }

    info!(path = %output_path.display(), "Downloading file");

    let resp = client
        .get(&download_url)
        .header("Cookie", cookie)
        .send()
        .await
        .wrap_err("Failed to start download")?;

    if !resp.status().is_success() {
        bail!("Download request failed with status: {}", resp.status());
    }

    let total_size = resp.content_length().unwrap_or(metadata.size);

    let pb = ProgressBar::new(total_size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")?
            .progress_chars("#>-"),
    );

    let mut file = File::create(&output_path)
        .await
        .wrap_err("Failed to create output file")?;

    let mut stream = resp.bytes_stream();
    let mut downloaded: u64 = 0;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.wrap_err("Error reading download stream")?;
        file.write_all(&chunk)
            .await
            .wrap_err("Failed to write to file")?;
        downloaded += chunk.len() as u64;
        pb.set_position(downloaded);
    }

    pb.finish_with_message("Download complete");

    // Verify file size
    let actual_size = tokio::fs::metadata(&output_path).await?.len();
    if actual_size == metadata.size {
        info!(size = %format_size(actual_size), "File size verified");
    } else {
        warn!(
            expected = metadata.size,
            actual = actual_size,
            "Size mismatch"
        );
    }

    info!(path = %output_path.display(), "Download complete");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    let args = Args::parse();

    // Validate URL
    Url::parse(&args.share_url).wrap_err("Invalid URL")?;

    let parts = parse_share_url(&args.share_url)?;

    let client = Client::builder()
        .cookie_store(true)
        .build()
        .wrap_err("Failed to create HTTP client")?;

    let (cookie, cookie_value) = get_session_cookie(&client, &args.share_url).await?;
    let metadata = get_file_metadata(&client, &parts).await?;

    println!();
    println!("File: {}", metadata.name);
    println!("Size: {}", format_size(metadata.size));
    println!();

    if args.info {
        info!("Info-only mode, skipping download");
        return Ok(());
    }

    download_file(
        &client,
        &parts,
        &metadata,
        &cookie,
        &cookie_value,
        &args.output_dir,
        args.force,
    )
    .await?;

    info!("Done!");
    Ok(())
}
