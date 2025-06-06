//! Guided setup and configuration wizard to make deployment as easy as possible

use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::anyhow;
use argon2::{password_hash::SaltString, Argon2, PasswordHasher};
use inquire::{validator::Validation, Confirm, CustomUserError, Password, Select, Text};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use tracing::Level;

use crate::*;

/// Global configuration for the entire application read from 'config.toml'.
///
/// Curious what all the options do? Simply go through the interactive setup wizard by invoking the
/// application with the '--init' flag and the displayed help text should answer your questions.
#[derive(Debug, Deserialize, Serialize)]
pub struct AppConfiguration {
    pub app_name: String,
    pub interface: String,
    pub proxy_depth: u64,
    pub admin_password_hash: String,
    pub maximum_filesize: u64,
    pub maximum_quota: u64,
    pub maximum_uploads_per_ip: u64,
    pub daily_request_limit_per_ip: u64,
    pub log_level: String,
    pub enable_privacy_policy: bool,
    pub enable_legal_notice: bool,
    pub demo_mode: bool,
}

impl AppConfiguration {
    /// Translate the log_level-String in the config.toml to the actual tracing::Level.
    /// Should that fail the app will simply fall back to INFO.
    pub fn translate_log_level(&self) -> Level {
        match self.log_level.as_str() {
            "ERROR" => Level::ERROR,
            "WARN" => Level::WARN,
            _ => Level::INFO,
        }
    }
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
        "K" => number.and_then(|n| n.checked_mul(1024)),
        "M" => number.and_then(|n| n.checked_mul(1024 * 1024)),
        "G" => number.and_then(|n| n.checked_mul(1024 * 1024 * 1024)),
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

/// Validator for 'inquire' to check that the maximum filesize input is valid.
///
/// This is similar to [validate_filesize_input] but also checks that the given filesize
/// stays at or below 2^39 - 256 bits = 68719476704 Bytes =~= 64GiB.
/// This is required by AES-GCM. If the message length exceeds that length the cypher
/// breaks down and loses its secure properties.
fn validate_max_filesize_input(input: &str) -> Result<Validation, CustomUserError> {
    match transform_filesize_input(input) {
        Some(v) => match v {
            0..=WEBCRYPTO_MAX_FILESIZE => Ok(Validation::Valid),
            _ => Ok(Validation::Invalid(
                "Filesize too large. Choose a value smaller or equal to 2GiB.".into(),
            )),
        },
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

/// The application's interactive configuration wizard started with the '--init' flag.
pub fn setup_config(config_path: &Path) -> Result<(), anyhow::Error> {
    eprintln!("Setting up new configuration at {config_path:?}");
    eprintln!("On setup completion, any previously present config file will be overwritten");
    eprintln!("Templates in '{DATA_PATH}/user_templates/' will remain untouched.");
    eprintln!("Interactively prompting for all settings ...\n");

    let app_name = Text::new("App name:")
        .with_initial_value("FerriShare")
        .with_help_message(
            "
  The name of the application.
  Will be displayed in the top-left corner of the interface.
",
        )
        .with_validator(inquire::validator::MinLengthValidator::new(1))
        .prompt()?;

    let interface = Text::new("Interface:")
        .with_initial_value("0.0.0.0:3000")
        .with_help_message(
            "
  The interface the server will listen on.

  Examples:
    127.0.0.1:8000 -> Serve only on localhost (port 8000)
      0.0.0.0:3000 -> Serve all incoming IPv4 connections (port 3000)

  Using Docker with a reverse-proxy? Just leave this untouched.
",
        )
        .prompt()?;

    let proxy_depth = Text::new("Proxy Depth:")
        .with_initial_value("1")
        .with_validator(|v: &str| {
            v.parse::<u64>()
                .map_or(Ok(Validation::Invalid("not a valid number".into())), |_| {
                    Ok(Validation::Valid)
                })
        })
        .with_help_message(
            "
  How many trusted reverse-proxies do requests pass through?

  The app performs IP-based rate-limiting. To do that correctly it needs to
  know which value to read from the request's 'X-Forwarded-For'-header.
  Can be set to 0 to directly use the client address for rate-limiting.

  Using the example Docker setup with Traefik? Set to '1'.
",
        )
        .prompt()?
        // Due to the validator this parse should never fail.
        .parse::<u64>()
        .unwrap();

    let admin_password = Password::new("Admin password:")
        .with_display_mode(inquire::PasswordDisplayMode::Masked)
        .with_help_message(
            "
  Site-wide administration password used at the '/admin'-URL.

  The admin panel allows you to view statistics on all uploaded files.
  It also allows you to delete files before they have expired.
  However, since the files are end-to-end-encrypted, you cannot download them.

  An argon2id-hash of your password will be stored in the generated config.
",
        )
        .prompt()?;

    let maximum_filesize = Text::new("Maximum filesize:")
        .with_initial_value("25M")
        .with_validator(validate_max_filesize_input)
        .with_formatter(&format_filesize_input)
        .with_help_message(
            "
  Maximum filesize that users can upload and store on the server.

  Due to limitations of the WebCrypto-API used on the frontend this
  is currently limited to 2GiB. Additionally, please bear in mind
  that uploaded files are first streamed to memory, hashed,
  and then stored on disk. This can cause problems if you accept
  files that are larger than your RAM.

  The prompt uses suffixes 'K', 'M' and 'G' which are read as binary suffixes:
    '250K' -> 250 KiB ->       256_000 Bytes
     '25M' ->  25 MiB ->    26_214_400 Bytes
      '1G' ->   1 GiB -> 1_073_741_824 Bytes
",
        )
        .prompt()?;

    let maximum_quota = Text::new("Maximum storage:")
        .with_initial_value("5G")
        .with_validator(validate_filesize_input)
        .with_formatter(&format_filesize_input)
        .with_help_message(
            "
  How much storage all uploaded files are at most allowed to consume.

  Once the remaining quota is less than the maximum allowed filesize,
  uploads will be disabled until old files have expired / were deleted.
  For example, if max_quota = 5 GiB and max_filesize = 500 MiB,
  then uploads would be disabled once on-disk storage exceeds 4.5 GiB.

  The prompt uses suffixes 'K', 'M' and 'G' which are read as binary suffixes:
    '250K' -> 250 KiB ->       256_000 Bytes
     '25M' ->  25 MiB ->    26_214_400 Bytes
      '5G' ->   5 GiB -> 5_368_709_120 Bytes
",
        )
        .prompt()?;

    let maximum_uploads_per_ip = Text::new("Maximum uploads per IP:")
        .with_initial_value("10")
        .with_validator(|v: &str| {
            v.parse::<u64>()
                .map_or(Ok(Validation::Invalid("not a valid number".into())), |_| {
                    Ok(Validation::Valid)
                })
        })
        .with_help_message(
            "
  How many uploaded files per IP address does the server permit?

  The database associates each upload with the IP address of the uploading client.
  If the number of uploads associated with a single IP reaches this threshold,
  they receive an error that they've already uploaded too many files and need
  to wait until old ones expire (or delete them manually).

  'IP address' here refers to either an IPv4 address or an IPv6 /64-subnet.
",
        )
        .prompt()?
        // Due to the validator this parse should never fail.
        .parse::<u64>()
        .unwrap();

    let daily_request_limit_per_ip = Text::new("Daily request limit per IP:")
        .with_initial_value("1000")
        .with_validator(|v: &str| {
            v.parse::<u64>()
                .map_or(Ok(Validation::Invalid("not a valid number".into())), |_| {
                    Ok(Validation::Valid)
                })
        })
        .with_help_message(
            "
  How many requests (GET and POST) can a single IP address make per day?

  Essentially a rate-limiter to ensure the server does not get DDoS'd.
  Uses a leaky bucket algorithm internally that decreases users' request counts
  every 15 minutes.

  'IP address' here refers to either an IPv4 address or an IPv6 /64-subnet.
",
        )
        .prompt()?
        // Due to the validator this parse should never fail.
        .parse::<u64>()
        .unwrap();

    let log_levels = vec![Level::INFO, Level::WARN, Level::ERROR];
    let log_level = Select::new("Log level:", log_levels)
        .with_help_message(
            "
  Set the log level of the entire application. (↑↓ to move, enter to select)
  Unless terse logs are somehow required it is recommended to set this to INFO.

  ERROR logs all internal server errors and failures.
  WARN logs suspicious client-side errors.
  INFO logs all HTTP responses and application events, including
  file creation/deletion/expiry and admin login/logout/session-expiry.
",
        )
        .prompt()?;

    let enable_privacy_policy = Confirm::new("Enable Privacy Policy?")
        .with_default(true)
        .with_help_message(
            "
  Some jurisdictions require the presence of a Privacy Policy.

  At `./data/user_templates/privacy_policy.html` a default privacy policy
  will be created that accurately describes what kind of data FerriShare
  collects during normal operation. You can edit this file freely.

  Choose Yes to serve this template and link to it in the application's footer.
  Choose No to not serve the template and remove the link from the footer.
",
        )
        .prompt()?;

    let enable_legal_notice = Confirm::new("Enable Legal Notice?")
        .with_default(false)
        .with_help_message(
            "
  Some jurisdictions require the presence of a Legal Notice.

  At `./data/user_templates/legal_notice.html` a Legal Notice stub will be
  created that you can edit and adjust to suit your needs.

  Choose Yes to serve this template and link to it in the application's footer.
  Choose No to not serve the template and remove the link from the footer.
",
        )
        .prompt()?;

    eprintln!("\nFinalizing configuration...");

    // Copy over the Privacy Policy if it doesn't already exist.
    let privacy_policy_path = format!("{DATA_PATH}/user_templates/privacy_policy.html");
    if PathBuf::from(privacy_policy_path.clone()).exists() {
        eprintln!("Found Privacy Policy at '{privacy_policy_path}', leaving untouched.");
    } else {
        std::fs::copy(
            "./templates/privacy_policy_default.html",
            &privacy_policy_path,
        )
        .map_err(|e| anyhow!("failed to copy privacy policy template: {e}"))?;
        eprintln!("Copied Privacy Policy template to '{privacy_policy_path}'.",);
    }

    // Copy over the Legal Notice if it doesn't already exist.
    let legal_notice_path = format!("{DATA_PATH}/user_templates/legal_notice.html");
    if PathBuf::from(legal_notice_path.clone()).exists() {
        eprintln!("Found Legal Notice at '{legal_notice_path}', leaving untouched.");
    } else {
        std::fs::copy("./templates/legal_notice_stub.html", &legal_notice_path)
            .map_err(|e| anyhow!("failed to copy legal notice template: {e}"))?;
        eprintln!("Copied Legal Notice template to '{legal_notice_path}'.");
    }

    // Perform postprocessing on the given answers.
    eprint!("Hashing password ...");

    // Turn the filesize strings into the actual byte counts.
    let maximum_filesize = transform_filesize_input(&maximum_filesize).unwrap();
    let maximum_quota = transform_filesize_input(&maximum_quota).unwrap();

    // Hash the admin password.
    // Use 32MB of memory and 4 iterations. That's a little stronger than the default parameters.
    let admin_password_hash = Argon2::new(
        argon2::Algorithm::default(),
        argon2::Version::default(),
        argon2::Params::new(32768, 4, 1, None).map_err(|e| anyhow!(e.to_string()))?,
    )
    .hash_password(admin_password.as_bytes(), &SaltString::generate(&mut OsRng))
    .map_err(|e| anyhow!(e.to_string()))?
    .to_string();

    eprintln!(" done!");

    // Bring it all together.
    let app_config = AppConfiguration {
        app_name,
        interface,
        proxy_depth,
        admin_password_hash,
        maximum_filesize,
        maximum_quota,
        maximum_uploads_per_ip,
        daily_request_limit_per_ip,
        log_level: log_level.to_string(),
        enable_privacy_policy,
        enable_legal_notice,
        demo_mode: false,
    };

    // Serialize to TOML and write to disk as 'config.toml'.
    File::create(config_path)?.write_all(toml::to_string(&app_config)?.as_bytes())?;

    eprintln!("Successfully wrote config to {config_path:?}.");
    eprintln!("You can now launch the app.");

    Ok(())
}
