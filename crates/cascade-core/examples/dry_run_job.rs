//! End-to-end demo of the core pipeline, with **zero risk**:
//!
//! 1. create two temp dirs with sample files,
//! 2. classify the operation's risk,
//! 3. build an rsync **dry-run** argv (no shell),
//! 4. run it through the async process runner,
//! 5. print the sanitized, live-streamed output.
//!
//! Run with:  `cargo run -p cascade-core --example dry_run_job`
//!
//! Uses rsync (already installed). The exact same flow drives rclone — swap the
//! builder for `rclone::command::build_args`.

use cascade_core::process::{spawn, ProcessEvent};
use cascade_core::rclone::command::preview;
use cascade_core::rsync::{build_args, RsyncOptions};
use cascade_core::security::destructive::{classify, Operation};
use cascade_core::security::path;

fn main() -> std::io::Result<()> {
    // 1. Sample source/destination.
    let src = std::env::temp_dir().join("cascade_demo_src");
    let dst = std::env::temp_dir().join("cascade_demo_dst");
    std::fs::create_dir_all(&src)?;
    std::fs::create_dir_all(&dst)?;
    std::fs::write(src.join("photo1.jpg"), b"fake-image-1")?;
    std::fs::write(src.join("photo2.jpg"), b"fake-image-2")?;
    std::fs::write(src.join("notes.txt"), b"hello")?;

    let source = format!("{}/", src.display());
    let dest = format!("{}/", dst.display());

    // 2. Validate paths + classify risk.
    println!("== Path validation ==");
    println!("  source: {:?}", path::validate(&source).unwrap());
    println!("  dest:   {:?}", path::validate(&dest).unwrap());

    let risk = classify(Operation::Copy, false);
    println!(
        "  risk:   {risk:?} (dry-run recommended: {})",
        risk.recommends_dry_run()
    );

    // 3. Build a DRY-RUN argv. Nothing is ever written.
    let opts = RsyncOptions {
        dry_run: true,
        excludes: vec!["*.txt".into()],
        ..Default::default()
    };
    let args = build_args(&source, &dest, &opts).expect("valid command");

    println!("\n== Command preview (display only) ==");
    println!("  {}", preview("rsync", &args));

    // 4. Run it through the async runner and stream events live.
    println!("\n== Live output ==");
    let handle = spawn("rsync", args);
    while let Ok(ev) = handle.events.recv_blocking() {
        match ev {
            ProcessEvent::Started { pid } => println!("  [started pid={pid:?}]"),
            ProcessEvent::Stdout(line) => println!("  {line}"),
            ProcessEvent::Stderr(line) => eprintln!("  ! {line}"),
            ProcessEvent::Error(e) => eprintln!("  [error] {e}"),
            ProcessEvent::Finished { success, code } => {
                println!("  [finished success={success} code={code:?}]");
                break;
            }
        }
    }

    println!("\nDry-run complete — destination was NOT modified.");
    Ok(())
}
