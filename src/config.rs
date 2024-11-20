use inquire::{validator::Validation, CustomUserError, Password, Select, Text};
use serde::{Deserialize, Serialize};
use tracing::Level;

/// Configuration for the entire application read from 'config.toml'.
#[derive(Debug, Deserialize, Serialize)]
struct AppConfiguration {
    interface: String,
    admin_password_hash: String,
    maximum_filesize: u64,
    maximum_quota: u64,
    log_level: String,
    demo_mode: bool,
}

/// Translate a filesize string to the actual number of bytes it represents.
///
/// The prompt uses suffixes 'K', 'M' and 'G' which are read as binary suffixes:
///    '25M' ->  25 MiB ->    26_214_400 Bytes
///   '250K' -> 250 KiB ->       256_000 Bytes
///     '5G' ->   5 GiB -> 5_368_709_120 Bytes
fn transform_filesize_input(input: &str) -> Option<u64> {
    // Split the string into number and suffix.
    let (number_str, suffix) = input.split_at(input.len() - 1);
    // Try to parse the number.
    let number = number_str.parse::<u64>().ok();
    // Next, try to parse the suffix and return the actual byte value.
    match suffix {
        "K" => number.map(|n| n.checked_mul(1024)).flatten(),
        "M" => number.map(|n| n.checked_mul(1024 * 1024)).flatten(),
        "G" => number.map(|n| n.checked_mul(1024 * 1024 * 1024)).flatten(),
        _ => None,
    }
}

/// Validator for 'inquire' to check that the filesize input is valid.
fn validate_filesize_input(input: &str) -> Result<Validation, CustomUserError> {
    match transform_filesize_input(input) {
        Some(_) => Ok(Validation::Valid),
        None => Ok(Validation::Invalid(
            "Failed to parse filesize. Use values like '100K', '25M' or '5G'.".into(),
        )),
    }
}

/// Formats filesize input such as '25M' as '25M = 26214400 Bytes'.
fn format_filesize_input(input: &str) -> String {
    format!(
        "{} = {} Bytes",
        input,
        transform_filesize_input(input).unwrap_or_default()
    )
}

pub fn setup_config() {
    // TODO Check if a cfg already exists.

    eprintln!("Setting up new configuration!");
    eprintln!("You will now be prompted for all settings ...\n");

    let interface = Text::new("Interface:")
        .with_initial_value("0.0.0.0:3000")
        .with_help_message(
            "
  The interface the server will listen on.

  Examples:
    127.0.0.1:8000 -> Serve only for localhost (port 8000)
      0.0.0.0:3000 -> Serve all incoming IPv4 connections (port 3000)

  Using Docker with a reverse-proxy? Just leave this untouched.
",
        )
        .prompt();

    let admin_password = Password::new("Admin Password:")
        .with_display_mode(inquire::PasswordDisplayMode::Masked)
        .with_help_message(
            "
  Site-wide administration password used at the '/admin'-URL.

  There is only one admin password for the entire application.
  To keep things simple there are no usernames, e-mail addresses, etc.

  The admin panel allows you to view statistics on all uploaded files.
  It also allows you to delete those files before they have expired.
  Since the files are end-to-end-encrypted, you cannot download them.

  This config-utility will create and store an argon2id-hash of your password.
",
        )
        .prompt();

    let max_filesize = Text::new("Maximum Filesize:")
        .with_initial_value("25M")
        .with_validator(validate_filesize_input)
        .with_formatter(&format_filesize_input)
        .with_help_message(
            "
  Maximum filesize that users can upload and store on the server.

  Please bear in mind that uploaded files are first streamed to memory,
  hashed, and then stored on disk. This can cause problems if you
  accept files that are larger than your RAM.

  The prompt uses suffixes 'K', 'M' and 'G' which are read as binary suffixes:
     '25M' ->  25 MiB ->    26_214_400 Bytes
    '250K' -> 250 KiB ->       256_000 Bytes
      '5G' ->   5 GiB -> 5_368_709_120 Bytes
",
        )
        .prompt();

    let max_quota = Text::new("Maximum Storage:")
        .with_initial_value("5G")
        .with_validator(validate_filesize_input)
        .with_formatter(&format_filesize_input)
        .with_help_message(
            "
  How much storage all uploaded files are at most allowed to consume.

  Once this limit has been reached users will not be able to upload
  more files until old ones have expired and are cleared from disk.

  The prompt uses suffixes 'K', 'M' and 'G' which are read as binary suffixes:
     '25M' ->  25 MiB ->    26_214_400 Bytes
    '250K' -> 250 KiB ->       256_000 Bytes
      '5G' ->   5 GiB -> 5_368_709_120 Bytes
",
        )
        .prompt();

    let log_levels = vec![Level::INFO, Level::WARN, Level::ERROR];
    let log_level = Select::new("Log level:", log_levels)
        .with_help_message(
            "
  Set the log level of the entire application.
  Unless terse logs are somehow required it is recommended to set this to INFO.

  ERROR logs all internal server errors and failures:
  - Failures to read from / write to the database.
  - Failes to read from / write to disk storage.
  - Failures to parse / process values that reside in the database.

  WARN includes ERROR and additionally logs suspicious client-side errors:
  - Malicious or malformed requests 
  - Unauthorized requests on non-user facing endpoints

  INFO includes WARN+ERROR and logs all HTTP requests and responses.
  Additionally, all notable events on the application are logged separately:
  - A file is uploaded by a user
  - A file is deleted (either manually by a user/admin or b/c it expired)
  - An admin has logged in
  - An admin is logged out (either manually or automatically)
",
        )
        .prompt();
}

// TODO:
// interface (with examples)
// max filesize (with suffix and examples)
// total quoate (with suffix and examples)
// admin-password (with confirmation)
// reverse-proxy settings (no idea yet)
// log-level (maybe)
