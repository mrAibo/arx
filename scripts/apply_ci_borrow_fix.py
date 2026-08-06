from pathlib import Path

path = Path("src/tui.rs")
text = path.read_text()
needle = """                    let pane = state.active_pane_mut();\n\n                    match key.code {\n"""
replacement = """                    // Handle tree-filter Backspace before borrowing the active pane.\n                    if key.code == KeyCode::Backspace && state.show_tree {\n                        state.tree_filter.pop();\n                        continue;\n                    }\n\n                    let pane = state.active_pane_mut();\n\n                    match key.code {\n"""
old_block = """                        KeyCode::Backspace => {\n                            // Tree filter: pop last char\n                            if state.show_tree {\n                                state.tree_filter.pop();\n                                continue;\n                            }\n                            let go_back = match &pane.location {\n"""
new_block = """                        KeyCode::Backspace => {\n                            let go_back = match &pane.location {\n"""

if needle not in text:
    raise SystemExit("pane borrow anchor not found")
if old_block not in text:
    raise SystemExit("Backspace block anchor not found")

text = text.replace(needle, replacement, 1)
text = text.replace(old_block, new_block, 1)
path.write_text(text)
