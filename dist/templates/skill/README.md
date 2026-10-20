# __TITLE__

An IYAKKI skill: plain text the assistant reads when a request matches the trigger words. It runs nothing and asks
for no access.

Files
* `skill.md`: the skill (front matter with name, description, triggers; then the body, at most 3950 characters, ASCII).
* `iyakki-app.json`: what the store shows (title, summary, licence, permissions).
* `tests/test_skill.py`: checks the skill parses and fits (`python3 tests/test_skill.py`).

Ship it
1. Edit `skill.md`; keep it short, with an example.
2. `python3 tests/test_skill.py`
3. `iyakki pkg validate .`
4. `iyakki pkg keygen me` once (keep `me.key` private), then `iyakki pkg pack . --key me.key -o __NAME__.iyp`
5. `iyakki pkg verify __NAME__.iyp`, then `iyakki pkg publish --server URL --token-env STORE_TOKEN __NAME__.iyp`

Raise `version` in `iyakki-app.json` for every release: versions only go up.
