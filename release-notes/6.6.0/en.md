This release adds a step-by-step setup wizard and adjustable desktop UI zoom for easier setup and a more compact workspace, plus a new Cheaper Inference provider.

## Features
- **Setup wizard**: Choose your language and appearance, accept the disclaimer, add a subscription, select models, test the connection, and connect AI tools, then review a setup summary.
  - Continue from the full subscription form or a backup import. After accepting the disclaimer, you can defer setup and resume from the sidebar, or reopen the wizard in Settings.
  - Includes writing Claude Code settings, copying Codex configuration, and links to setup guides for other tools.
- **Desktop UI zoom**: Choose 80%, 90%, 100%, 110%, or 125% in Settings → General → Appearance. Changes apply immediately and persist; the default and reset value is 90%.
- **Cheaper Inference provider** (#65, thanks @aiapienthusiast): Adds an Anthropic Messages-compatible endpoint, API key authentication, and model discovery. Compatibility remains marked as untested.

## Fixes
- **Compact window layouts**: The setup wizard button in Settings no longer fills the entire row. The request log table keeps its minimum width and scrolls horizontally within the table to reveal the rightmost columns; long names and large values no longer disrupt the layout.

## Other
- **More compact default desktop window**: The initial width is now 1280, with small-screen adaptation preserved. Windows no longer applies extra automatic magnification, so the UI consistently uses the selected zoom level.
- **README feature showcase**: A time-based virtual models section follows Terminal UI, with schedule settings and routing preview screenshots. Both Chinese READMEs use Chinese screenshots; the other languages use English screenshots.
