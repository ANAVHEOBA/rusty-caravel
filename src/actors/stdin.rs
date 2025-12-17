use super::monitor::MonitorHandle;
use shell_words;
use tokio::sync::mpsc;

use clap::{AppSettings, Parser};

use log::info;

/// Convert a hex string (without 0x prefix) to bytes
fn hex_string_to_bytes(hex: &str) -> Result<Vec<u8>, String> {
    if hex.is_empty() {
        return Err("Empty hex string".into());
    }
    // Handle odd-length hex strings by padding with leading zero
    let padded = if hex.len() % 2 != 0 {
        format!("0{}", hex)
    } else {
        hex.to_string()
    };

    (0..padded.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&padded[i..i + 2], 16)
                .map_err(|_| format!("Invalid hex character in: {}", hex))
        })
        .collect()
}

/// Parse CAN ID from string (supports decimal and hex with 0x prefix)
fn parse_can_id(input: &str) -> Result<u32, String> {
    let input = input.trim();
    if input.starts_with("0x") || input.starts_with("0X") {
        u32::from_str_radix(&input[2..], 16)
            .map_err(|_| format!("Invalid hex CAN ID: {}", input))
    } else {
        input
            .parse()
            .map_err(|_| format!("Invalid CAN ID: {}", input))
    }
}

/// Parse message from string into bytes
/// Supports:
/// - UTF-8 strings in quotes: "Hello" or 'Hello'
/// - Hexadecimal with 0x prefix: 0xDEADBEEF
/// - Decimal numbers: 12345
fn parse_message(input: &str) -> Result<Vec<u8>, String> {
    let input = input.trim();

    // 1. Check for quoted string (UTF-8)
    if (input.starts_with('"') && input.ends_with('"'))
        || (input.starts_with('\'') && input.ends_with('\''))
    {
        if input.len() < 2 {
            return Err("Empty quoted string".into());
        }
        let inner = &input[1..input.len() - 1];
        let bytes = inner.as_bytes();
        if bytes.len() > 8 {
            return Err(format!(
                "Message too long: {} bytes (max 8 for CAN)",
                bytes.len()
            ));
        }
        return Ok(bytes.to_vec());
    }

    // 2. Check for hex prefix (0x or 0X)
    if input.starts_with("0x") || input.starts_with("0X") {
        let hex_str = &input[2..];
        let bytes = hex_string_to_bytes(hex_str)?;
        if bytes.len() > 8 {
            return Err(format!(
                "Message too long: {} bytes (max 8 for CAN)",
                bytes.len()
            ));
        }
        return Ok(bytes);
    }

    // 3. Parse as decimal number
    let num: u64 = input
        .parse()
        .map_err(|_| format!("Invalid message format: {}", input))?;

    // Convert to minimal byte representation (trim leading zeros)
    let bytes = num.to_be_bytes();
    let first_nonzero = bytes.iter().position(|&b| b != 0).unwrap_or(7);
    Ok(bytes[first_nonzero..].to_vec())
}

#[derive(Parser)]
#[clap(version = "0.2.0", author = "marujos", setting=AppSettings::NoBinaryName)]
pub struct Opts {
    /// Sets a custom config file. Could have been an Option<T> with no default too
    #[clap(short, long)]
    config: Option<String>,
    #[clap(subcommand)]
    subcmd: SubCommand,
}

#[derive(Parser)]
enum SubCommand {
    Send(Send),
    Receive(Receive),
    Exit(Exit),
}

#[derive(Parser)]
struct Send {
    id: String,
    message: String,
    cycletime: String,
}

#[derive(Parser)]
struct Exit {}

#[derive(Parser)]
struct Receive {
    id: Option<String>,
    nr_of_messages: Option<String>,
}

#[derive(Debug)]
enum Messages {
    Line { line: String },
    Shutdown,
}

struct StdInLines {
    inbox: mpsc::Receiver<Messages>,
    monitor: MonitorHandle,
}

impl StdInLines {
    fn new(inbox: mpsc::Receiver<Messages>, monitor: MonitorHandle) -> Self {
        StdInLines { inbox, monitor }
    }

    async fn tell_monitor(&self) {
        self.monitor.exit_received().await.unwrap();
    }

    async fn handle_message(&mut self, msg: Messages) -> bool {
        match msg {
            Messages::Shutdown => false,
            Messages::Line { line } => self.handle_command(line).await,
        }
    }

    async fn handle_command(&mut self, msg: String) -> bool {
        let words = shell_words::split(&msg).expect("cmd split went bust");

        let cmd: Opts = match Opts::try_parse_from(words) {
            Ok(opts) => opts,
            Err(error) => {
                println!("{}", error);
                return true;
            }
        };

        match cmd.subcmd {
            SubCommand::Send(t) => {
                // Parse CAN ID (supports hex with 0x prefix or decimal)
                let id = match parse_can_id(&t.id) {
                    Ok(id) => id,
                    Err(e) => {
                        println!("Error: {}", e);
                        return true;
                    }
                };

                // Parse message (supports UTF-8 strings, hex, or decimal)
                let message = match parse_message(&t.message) {
                    Ok(msg) => msg,
                    Err(e) => {
                        println!("Error: {}", e);
                        return true;
                    }
                };

                // Parse cycletime
                let cycletime: u64 = t.cycletime.parse().unwrap_or(0);

                // Log the parsed message
                println!(
                    "CAN TX: ID=0x{:X} Data=[{}] Cycle={}ms",
                    id,
                    message
                        .iter()
                        .map(|b| format!("{:02X}", b))
                        .collect::<Vec<_>>()
                        .join(" "),
                    cycletime
                );

                // TODO: Wire up to sender actor when enabled
                // if cycletime == 0 {
                //     self.sender.send_can_message(id, message, cycletime).await;
                // } else {
                //     tokio::spawn(cyclic_sender(self.sender.clone(), id, message, cycletime));
                // }
                true
            }
            SubCommand::Receive(t) => {
                //self.receiver.receive_can_msg(t.id, t.nr_of_messages).await;
                true
            }
            SubCommand::Exit(_t) => {
                self.tell_monitor().await;
                false
            }
        }
    }
}

//async fn cyclic_sender(sender: SenderCANHandle, id: u32, message: u64, cycletime: u64) {
//    loop {
//        sleep(Duration::from_millis(cycletime)).await;
//        sender.send_can_message(id, message, cycletime).await
//    }
//}

async fn run(mut actor: StdInLines) {
    info!("Running");

    while let Some(msg) = actor.inbox.recv().await {
        if !actor.handle_message(msg).await {
            break;
        }
    }

    info!("Shutting Down");
}

fn reading_stdin_lines(sender: mpsc::Sender<Messages>) {
    let runtime = tokio::runtime::Handle::current();
    std::thread::spawn(move || {
        let sender = sender.clone();
        let stdin = std::io::stdin();
        let mut line_buf = String::new();
        while let Ok(_) = stdin.read_line(&mut line_buf) {
            let sender = sender.clone();
            let line = line_buf.trim_end().to_string();
            line_buf.clear();

            runtime.spawn(async move {
                let message = Messages::Line { line };
                let result = sender.send(message).await;
                if let Err(error) = result {
                    println!("start_reading_stdin_lines send error: {:?}", error);
                }
            });
        }
    });
}

pub struct StdInLinesHandle {
    inbox: mpsc::Sender<Messages>,
}

impl StdInLinesHandle {
    pub fn new(
        // runtime: tokio::runtime::Handle,
        //watch_receiver: CtrlCActorHandle,
        //sender: SenderCANHandle,
        //receiver: ReceiverCANHandle
        monitor: MonitorHandle,
    ) -> StdInLinesHandle {
        let (tx, inbox) = tokio::sync::mpsc::channel(5);

        reading_stdin_lines(tx.clone());

        let actor = StdInLines::new(inbox, monitor);

        tokio::spawn(run(actor));

        Self { inbox: tx }
    }

    pub async fn shutdown(&self) {
        let msg = Messages::Shutdown;

        self.inbox.try_send(msg).expect("What ?");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests for hex_string_to_bytes
    mod hex_string_to_bytes_tests {
        use super::*;

        #[test]
        fn converts_valid_hex() {
            assert_eq!(hex_string_to_bytes("FF").unwrap(), vec![0xFF]);
            assert_eq!(hex_string_to_bytes("DEADBEEF").unwrap(), vec![0xDE, 0xAD, 0xBE, 0xEF]);
            assert_eq!(hex_string_to_bytes("00").unwrap(), vec![0x00]);
        }

        #[test]
        fn handles_lowercase_hex() {
            assert_eq!(hex_string_to_bytes("deadbeef").unwrap(), vec![0xDE, 0xAD, 0xBE, 0xEF]);
            assert_eq!(hex_string_to_bytes("aB").unwrap(), vec![0xAB]);
        }

        #[test]
        fn pads_odd_length_hex() {
            assert_eq!(hex_string_to_bytes("F").unwrap(), vec![0x0F]);
            assert_eq!(hex_string_to_bytes("123").unwrap(), vec![0x01, 0x23]);
        }

        #[test]
        fn rejects_empty_string() {
            assert!(hex_string_to_bytes("").is_err());
        }

        #[test]
        fn rejects_invalid_hex_chars() {
            assert!(hex_string_to_bytes("GG").is_err());
            assert!(hex_string_to_bytes("ZZZZ").is_err());
            assert!(hex_string_to_bytes("12G4").is_err());
        }
    }

    // Tests for parse_can_id
    mod parse_can_id_tests {
        use super::*;

        #[test]
        fn parses_decimal_id() {
            assert_eq!(parse_can_id("123").unwrap(), 123);
            assert_eq!(parse_can_id("0").unwrap(), 0);
            assert_eq!(parse_can_id("2047").unwrap(), 2047); // Max 11-bit CAN ID
        }

        #[test]
        fn parses_hex_id_with_0x_prefix() {
            assert_eq!(parse_can_id("0x123").unwrap(), 0x123);
            assert_eq!(parse_can_id("0X123").unwrap(), 0x123);
            assert_eq!(parse_can_id("0xFF").unwrap(), 255);
            assert_eq!(parse_can_id("0x7FF").unwrap(), 2047);
        }

        #[test]
        fn handles_whitespace() {
            assert_eq!(parse_can_id(" 123 ").unwrap(), 123);
            assert_eq!(parse_can_id(" 0x123 ").unwrap(), 0x123);
        }

        #[test]
        fn rejects_invalid_id() {
            assert!(parse_can_id("abc").is_err());
            assert!(parse_can_id("").is_err());
            assert!(parse_can_id("0x").is_err());
            assert!(parse_can_id("0xGGG").is_err());
        }
    }

    // Tests for parse_message
    mod parse_message_tests {
        use super::*;

        // UTF-8 string tests
        #[test]
        fn parses_double_quoted_string() {
            assert_eq!(parse_message("\"Hello\"").unwrap(), b"Hello".to_vec());
            assert_eq!(parse_message("\"AB\"").unwrap(), b"AB".to_vec());
        }

        #[test]
        fn parses_single_quoted_string() {
            assert_eq!(parse_message("'Hello'").unwrap(), b"Hello".to_vec());
        }

        #[test]
        fn rejects_string_over_8_bytes() {
            assert!(parse_message("\"123456789\"").is_err()); // 9 bytes
            assert!(parse_message("\"HelloWorld\"").is_err()); // 10 bytes
        }

        #[test]
        fn accepts_string_at_8_bytes() {
            assert_eq!(parse_message("\"12345678\"").unwrap(), b"12345678".to_vec());
        }

        // Hexadecimal tests
        #[test]
        fn parses_hex_message() {
            assert_eq!(parse_message("0xDEADBEEF").unwrap(), vec![0xDE, 0xAD, 0xBE, 0xEF]);
            assert_eq!(parse_message("0xFF").unwrap(), vec![0xFF]);
            assert_eq!(parse_message("0X1234").unwrap(), vec![0x12, 0x34]);
        }

        #[test]
        fn rejects_hex_over_8_bytes() {
            assert!(parse_message("0x112233445566778899").is_err()); // 9 bytes
        }

        #[test]
        fn accepts_hex_at_8_bytes() {
            assert_eq!(
                parse_message("0x1122334455667788").unwrap(),
                vec![0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]
            );
        }

        // Decimal tests
        #[test]
        fn parses_decimal_message() {
            assert_eq!(parse_message("255").unwrap(), vec![0xFF]);
            assert_eq!(parse_message("256").unwrap(), vec![0x01, 0x00]);
            assert_eq!(parse_message("65535").unwrap(), vec![0xFF, 0xFF]);
        }

        #[test]
        fn parses_zero() {
            assert_eq!(parse_message("0").unwrap(), vec![0x00]);
        }

        #[test]
        fn trims_leading_zeros_for_decimal() {
            // 123 = 0x7B, should be single byte not 8 bytes
            let result = parse_message("123").unwrap();
            assert_eq!(result, vec![0x7B]);
        }

        // Edge cases
        #[test]
        fn handles_whitespace() {
            assert_eq!(parse_message(" 0xFF ").unwrap(), vec![0xFF]);
            assert_eq!(parse_message(" 123 ").unwrap(), vec![0x7B]);
        }

        #[test]
        fn rejects_invalid_format() {
            assert!(parse_message("not_a_number").is_err());
            assert!(parse_message("").is_err());
        }
    }
}
