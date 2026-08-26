from pathlib import Path
import subprocess

BASE = "3d8050829bf57866dd494a69ac5b0f5ebc9496d9"

cargo = Path("Cargo.toml")
text = cargo.read_text()
old = 'tokio-util = { version = "=0.7.16", features = ["io"] }'
new = 'tokio-util = { version = "0.7", features = ["io"] }'
if text.count(old) != 1:
    raise SystemExit("expected one exact tokio-util pin")
cargo.write_text(text.replace(old, new, 1))

base_lock = subprocess.check_output(["git", "show", f"{BASE}:Cargo.lock"], text=True)
Path("Cargo.lock").write_text(base_lock)
print("restored baseline lock and relaxed direct tokio-util constraint")
