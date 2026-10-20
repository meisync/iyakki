---
name: iyakki-skill-author
description: Write a good IYAKKI skill: choose triggers, keep the body short and concrete, add an example, respect the size limit, test that it parses, package and publish it
triggers: write a skill, new skill, skill file, skill triggers, teach the assistant
---
Use this when the person wants the assistant to learn a procedure, a style or house rules.

1. A skill is a Markdown file: a header between --- lines with name, description, triggers; then the body. The assistant reads the body only when the request matches a trigger word, so it must be short and directly useful. Hard limits: body at most 3950 characters, plain ASCII, at most 12 triggers.
2. Header: name is lowercase letters, digits, dashes (meeting-notes). description is one line saying WHEN to use it. triggers is a comma-separated list of the words people actually say (meeting, minutes, action items): specific words, not generic ones such as help or do.
3. Body shape: one opening line (Use this when ...), then numbered steps in the order to do them, each one imperative and concrete. Say which tools to use by name (read_file, calendar_add, web_search). State the output shape (a table, three bullets, a code block). Put hard rules last: what never to do.
4. Add one short example: a typical request and the answer in the wanted shape. An example teaches more than a paragraph of adjectives.
5. Do not paste large reference text: link the idea, give the rule. If it does not fit in 3950 characters, split it into two skills with different triggers.
6. Never include secrets, personal data or real customer names. Never tell the assistant to skip approvals or to run things silently.
7. Scaffold with app_scaffold {kind: skill, name, dir}; edit skill.md; run `python3 tests/test_skill.py` (checks header, name, size, ASCII, secrets); then store_validate the folder.
8. Try it: STORE, Settings, install from a file after `iyakki pkg pack`, or add it in SKILLS; then ask something that should trigger it and check the answer follows the steps. If it does not trigger, add the words people used. If it triggers too often, remove generic words.
9. Package and share with the steps of iyakki-app-author (pack, sign, verify, store_publish). Bump version for each change.
10. Track multi-step work on the feature board (board_list/board_add/board_update, tests=, files=; done only after tests pass). ACT: write files and run builds and tests yourself; never ask the person to. Bugs/failures: own board row with a B- id (never F-); bug_report (failing pods) captures logs; fix, verify, attach the passing run.
