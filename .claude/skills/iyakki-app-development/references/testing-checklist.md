# Testing checklist (tick each one)

1. `cargo test` passes in the app (golden files committed after reading them).
2. Three sizes (360x740, 800x480, 1280x720), text 1x and 3x: names, inside the pane, targets >= 44 on touch. (generated test; extend its `SIZES` if you add panes: record each pane at each size)
3. Each pane has a test that switches to it and checks one visible text.
4. Taps go through `h.mouse(x, y, Phase::Down/Up, t)` on controls found by name; keys through `h.event(&UiEvent::Key(..))`.
5. Persistence: a second app built over the same `MemStorage` sees the data; a refusing port shows its sentence.
6. Every tool: `ask` is right; the change waits for Allow; the read is free.
7. Every embedded view: its fake port is given; one test drives `Tick` then an `Action`.
8. A narrow pane and 3x text: a test with `h.touch = true; h.text_scale = 3.0;`.
9. `./run.sh app-check APP` prints no problems.
10. Inside the repo: `cargo test -p iyakki-core --test boundary --test dependency_ratchet --test ui_agent_uses --test feature_matrix --test kernel_single_reader`.
