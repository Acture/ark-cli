use std::ffi::OsStr;
use std::fs;
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use crate::config::Config;
use anyhow::{bail, Context, Result};
use clap::Args;
use glob::Pattern;
use md5::{Digest, Md5};
use rayon::prelude::*;
use reqwest::blocking::{Client, Response};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use zip::ZipArchive;

const USER_AGENT: &str = concat!(
	"ark-ingest/",
	env!("CARGO_PKG_VERSION"),
	" (+https://github.com/Acture/ark-cli)"
);
const CONNECT_TIMEOUT_SECS: u64 = 30;
const MANIFEST_TIMEOUT_SECS: u64 = 30;
const DOWNLOAD_TIMEOUT_SECS: u64 = 120;
const MAX_ATTEMPTS: usize = 3;

#[derive(Args, Debug, Clone)]
pub struct FetchArgs {
	/// Glob filter against logical manifest names (for example `chararts/char_*_amiya.ab`).
	#[arg(long, conflicts_with = "all", required_unless_present = "all")]
	pub filter: Option<String>,
	/// Output directory for restored `.ab` files.
	#[arg(long, value_name = "DIR")]
	pub to: Option<PathBuf>,
	/// Print matching manifest entries without downloading anything.
	#[arg(long)]
	pub dry_run: bool,
	/// Number of parallel download jobs.
	#[arg(long, default_value_t = NonZeroUsize::new(4).expect("non-zero default"))]
	pub jobs: NonZeroUsize,
	/// Overwrite existing files even if their MD5 already matches.
	#[arg(long)]
	pub force: bool,
	/// Match every manifest entry.
	#[arg(long, conflicts_with = "filter")]
	pub all: bool,
}

#[derive(Debug, Deserialize)]
struct VersionResponse {
	#[serde(rename = "resVersion")]
	res_version: String,
	#[serde(rename = "clientVersion")]
	_client_version: String,
}

#[derive(Debug, Deserialize)]
struct HotUpdateManifest {
	#[serde(rename = "abInfos")]
	ab_infos: Vec<HotUpdateEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct HotUpdateEntry {
	name: String,
	md5: String,
	#[serde(rename = "totalSize")]
	total_size: u64,
	#[serde(rename = "abSize")]
	ab_size: Option<u64>,
}

impl HotUpdateEntry {
	fn output_size(&self) -> u64 {
		self.ab_size.unwrap_or(self.total_size)
	}
}

pub fn run(args: FetchArgs) -> Result<()> {
	let output_root: PathBuf = match args.to.as_deref() {
		Some(output) => absolute_path(output)?,
		None => Config::load()
			.context("load rogue config for fetch output")?
			.hot_update_dir(),
	};
	let client: Client = build_client()?;
	let version: VersionResponse = fetch_json(
		&client,
		"https://ak-conf.hypergryph.com/config/prod/official/Android/version",
		Duration::from_secs(MANIFEST_TIMEOUT_SECS),
	)?;
	let manifest_url: String = format!(
		"https://ak.hycdn.cn/assetbundle/official/Android/assets/{}/hot_update_list.json",
		version.res_version
	);
	let manifest: HotUpdateManifest = fetch_json(
		&client,
		&manifest_url,
		Duration::from_secs(MANIFEST_TIMEOUT_SECS),
	)?;
	let filter: Option<Pattern> = if args.all {
		None
	} else {
		Some(
			Pattern::new(
				args.filter
					.as_deref()
					.context("--filter is required unless --all is passed")?,
			)
			.context("parse --filter glob")?,
		)
	};

	let mut matched: Vec<HotUpdateEntry> = manifest
		.ab_infos
		.into_iter()
		.filter(|entry| {
			filter
				.as_ref()
				.map(|pattern| pattern.matches(&entry.name))
				.unwrap_or(true)
		})
		.collect();
	matched.sort_by(|left, right| left.name.cmp(&right.name));
	if matched.is_empty() {
		bail!("no hot-update entries matched the requested filter");
	}

	let total_download_size: u64 = matched.iter().map(|entry| entry.total_size).sum();
	let total_output_size: u64 = matched.iter().map(HotUpdateEntry::output_size).sum();
	println!("resVersion: {}", version.res_version);
	println!(
		"matched {} file(s): {} output, {} download",
		matched.len(),
		format_bytes(total_output_size),
		format_bytes(total_download_size)
	);

	if args.dry_run {
		for entry in &matched {
			println!(
				"{}\t{}\t{}\t{}",
				entry.name,
				format_bytes(entry.output_size()),
				format_bytes(entry.total_size),
				entry.md5,
			);
		}
		println!(
			"total\t{}\t{}\t{} file(s)",
			format_bytes(total_output_size),
			format_bytes(total_download_size),
			matched.len()
		);
		return Ok(());
	}

	fs::create_dir_all(&output_root)
		.with_context(|| format!("create {}", output_root.display()))?;
	let pool = rayon::ThreadPoolBuilder::new()
		.num_threads(args.jobs.get())
		.build()
		.context("build rayon thread pool")?;
	pool.install(|| -> Result<()> {
		matched.par_iter().try_for_each(|entry| {
			download_entry(
				&client,
				&version.res_version,
				&output_root,
				entry,
				args.force,
			)
		})
	})?;

	println!(
		"downloaded {} file(s) into {}",
		matched.len(),
		output_root.display()
	);
	Ok(())
}

fn build_client() -> Result<Client> {
	Client::builder()
		.user_agent(USER_AGENT)
		.connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
		.build()
		.context("build reqwest client")
}

fn fetch_json<T>(client: &Client, url: &str, timeout: Duration) -> Result<T>
where
	T: DeserializeOwned,
{
	let response: Response = send_get_with_retries(client, url, timeout)?;
	response
		.json::<T>()
		.with_context(|| format!("parse JSON from {url}"))
}

fn send_get_with_retries(client: &Client, url: &str, timeout: Duration) -> Result<Response> {
	for attempt in 1..=MAX_ATTEMPTS {
		match client.get(url).timeout(timeout).send() {
			Ok(response) => {
				let status: StatusCode = response.status();
				if status.is_success() {
					return Ok(response);
				}
				if status == StatusCode::NOT_FOUND {
					bail!("GET {url} returned 404");
				}
				if (status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error())
					&& attempt < MAX_ATTEMPTS
				{
					thread::sleep(backoff_delay(attempt));
					continue;
				}
				let body: String = response.text().unwrap_or_default();
				if body.trim().is_empty() {
					bail!("GET {url} returned {status}");
				}
				bail!("GET {url} returned {status}: {}", body.trim());
			}
			Err(error) => {
				if (error.is_timeout() || error.is_connect()) && attempt < MAX_ATTEMPTS {
					thread::sleep(backoff_delay(attempt));
					continue;
				}
				return Err(error).with_context(|| format!("GET {url}"));
			}
		}
	}
	bail!("GET {url} exhausted retries")
}

fn backoff_delay(attempt: usize) -> Duration {
	let factor: u64 = 1_u64 << (attempt.saturating_sub(1) as u32);
	Duration::from_millis(500 * factor)
}

fn download_entry(
	client: &Client,
	res_version: &str,
	output_root: &Path,
	entry: &HotUpdateEntry,
	force: bool,
) -> Result<()> {
	let destination: PathBuf = output_root.join(&entry.name);
	if destination.exists() && !destination.is_file() {
		bail!(
			"destination exists but is not a file: {}",
			destination.display()
		);
	}
	if !force && destination.is_file() {
		let existing_md5: String = md5_hex_for_path(&destination)?;
		if existing_md5.eq_ignore_ascii_case(&entry.md5) {
			eprintln!("[skip] {}", entry.name);
			return Ok(());
		}
	}
	if let Some(parent) = destination.parent() {
		fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
	}

	let download_tmp: PathBuf = partial_path(&destination, ".download.partial")?;
	let output_tmp: PathBuf = partial_path(&destination, ".partial")?;
	let url: String = format!(
		"https://ak.hycdn.cn/assetbundle/official/Android/assets/{}/{}",
		res_version,
		cdn_dat_name(&entry.name)
	);
	let result = (|| -> Result<()> {
		let mut response: Response =
			send_get_with_retries(client, &url, Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
				.with_context(|| format!("download {}", entry.name))?;
		let mut download_file: File = File::create(&download_tmp)
			.with_context(|| format!("create {}", download_tmp.display()))?;
		response
			.copy_to(&mut download_file)
			.with_context(|| format!("write {}", download_tmp.display()))?;
		drop(download_file);

		let download_reader: File = File::open(&download_tmp)
			.with_context(|| format!("open {}", download_tmp.display()))?;
		let mut archive: ZipArchive<File> = ZipArchive::new(download_reader)
			.with_context(|| format!("open {} as zip", download_tmp.display()))?;
		let archive_member: String = archive_member_name(&mut archive, &entry.name)?;
		let mut bundled_file = archive
			.by_name(&archive_member)
			.with_context(|| format!("read {archive_member} from {}", download_tmp.display()))?;
		let mut output_file: File = File::create(&output_tmp)
			.with_context(|| format!("create {}", output_tmp.display()))?;
		let mut hasher: Md5 = Md5::new();
		let mut buffer: [u8; 64 * 1024] = [0; 64 * 1024];
		loop {
			let read: usize = bundled_file.read(&mut buffer).with_context(|| {
				format!("read {archive_member} from {}", download_tmp.display())
			})?;
			if read == 0 {
				break;
			}
			output_file
				.write_all(&buffer[..read])
				.with_context(|| format!("write {}", output_tmp.display()))?;
			hasher.update(&buffer[..read]);
		}
		output_file
			.flush()
			.with_context(|| format!("flush {}", output_tmp.display()))?;

		let actual_md5: String = format!("{:x}", hasher.finalize());
		if !actual_md5.eq_ignore_ascii_case(&entry.md5) {
			bail!(
				"MD5 mismatch for {}: expected {}, got {}",
				entry.name,
				entry.md5,
				actual_md5
			);
		}

		fs::rename(&output_tmp, &destination).with_context(|| {
			format!(
				"rename {} to {}",
				output_tmp.display(),
				destination.display()
			)
		})?;
		if download_tmp.exists() {
			fs::remove_file(&download_tmp)
				.with_context(|| format!("remove {}", download_tmp.display()))?;
		}
		eprintln!("[ok] {}", entry.name);
		Ok(())
	})();

	if result.is_err() {
		if download_tmp.exists() {
			let _ = fs::remove_file(&download_tmp);
		}
		if output_tmp.exists() {
			let _ = fs::remove_file(&output_tmp);
		}
	}
	result
}

fn archive_member_name(archive: &mut ZipArchive<File>, logical_name: &str) -> Result<String> {
	let mut members: Vec<String> = Vec::new();
	for index in 0..archive.len() {
		let member = archive
			.by_index(index)
			.with_context(|| format!("inspect zip entry {index}"))?;
		if member.is_dir() {
			continue;
		}
		members.push(member.name().to_string());
	}
	if members.iter().any(|member| member == logical_name) {
		return Ok(logical_name.to_string());
	}
	if members.len() == 1 {
		return Ok(members.into_iter().next().expect("single archive member"));
	}
	bail!(
		"zip archive did not contain {logical_name}; found {} file(s)",
		members.len()
	)
}

fn cdn_dat_name(logical_name: &str) -> String {
	let normalized: String = logical_name
		.strip_suffix(".ab")
		.unwrap_or(logical_name)
		.replace('/', "_");
	format!("{normalized}.dat")
}

fn partial_path(destination: &Path, suffix: &str) -> Result<PathBuf> {
	let file_name: &str = destination
		.file_name()
		.and_then(OsStr::to_str)
		.context("destination has no UTF-8 file name")?;
	Ok(destination.with_file_name(format!(".{file_name}{suffix}")))
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
	if path.is_absolute() {
		return Ok(path.to_path_buf());
	}
	Ok(std::env::current_dir()
		.context("read current directory")?
		.join(path))
}

fn md5_hex_for_path(path: &Path) -> Result<String> {
	let file: File = File::open(path).with_context(|| format!("open {}", path.display()))?;
	let mut reader: BufReader<File> = BufReader::new(file);
	let mut hasher: Md5 = Md5::new();
	let mut buffer: [u8; 64 * 1024] = [0; 64 * 1024];
	loop {
		let read: usize = reader
			.read(&mut buffer)
			.with_context(|| format!("read {}", path.display()))?;
		if read == 0 {
			break;
		}
		hasher.update(&buffer[..read]);
	}
	Ok(format!("{:x}", hasher.finalize()))
}

fn format_bytes(bytes: u64) -> String {
	format!("{:.2} MiB", bytes as f64 / (1024.0 * 1024.0))
}

#[cfg(test)]
mod tests {
	use super::{cdn_dat_name, partial_path};
	use std::path::Path;

	#[test]
	fn cdn_name_matches_chararts_rule() {
		assert_eq!(
			cdn_dat_name("chararts/char_002_amiya.ab"),
			"chararts_char_002_amiya.dat"
		);
	}

	#[test]
	fn partial_path_uses_hidden_sibling() {
		let path = Path::new("client/hot_update/chararts/char_002_amiya.ab");
		assert_eq!(
			partial_path(path, ".partial").unwrap(),
			Path::new("client/hot_update/chararts/.char_002_amiya.ab.partial")
		);
	}
}
