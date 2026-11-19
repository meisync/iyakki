# Which recipe for which job

| Job | File (docs/sdk/recipes/) |
|---|---|
| tabs / panes | add_tabs.rs |
| list with selection, detail | list_selection.rs |
| a field and a Save button with validation | form_validation.rs |
| keep a number or text between runs | persist_storage.rs |
| my own shapes in the skin's colours | themed_drawing.rs |
| a tool the person must approve | approval_card.rs and embed_chat.rs |
| the assistant in my layout, with my tools | embed_chat.rs |
| files of the person | embed_files.rs |
| command line | embed_terminal.rs |
| text editor | embed_editor.rs |
| sound player | embed_audio.rs |
| camera | embed_camera.rs |
| a web page | embed_browser.rs |
| several screens, one focus, Tab order, paging | multi_screen.rs |
| a list as a chat tool argument | tool_list_args.rs |
| a barcode (Code 128, EAN-13, UPC-A) | barcode_label.rs |
| a setting with a chat tool | docs/sdk/recipes/README.md, last section, and `chat-tools.md` |

How to use a recipe: read the file, copy it into `src/`, rename the struct and the `id`, give it its own action base, fold its `event` and `view` arms into your app, run `app-check`. How they are tested: `crates/iyakki-ui-sdk/tests/docs_recipes.rs`.
