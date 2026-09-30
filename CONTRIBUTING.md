# Contributing to EzConsoleGit

Thank you for your interest in contributing to **EzConsoleGit**! We welcome community contributions, bug reports, feature requests, and suggestions.

## How to Contribute

### 1. Reporting Bugs
- Search existing issues to ensure the bug hasn't already been reported.
- Open a new issue with a clear title and description.
- Include reproduction steps, your OS, terminal emulator, git version, and terminal screenshots or logs if applicable.

### 2. Suggesting Features
- Open an issue labeled "Feature Request".
- Clearly explain the problem you are trying to solve and how the proposed feature would help.
- Provide workflow descriptions or terminal mockups if applicable.

### 3. Submitting Pull Requests
1. **Fork** the repository and create your feature branch:
   ```bash
   git checkout -b feature/amazing-feature
   ```
2. **Make your changes**:
   - Follow standard Rust idiomatic guidelines and formatting (`cargo fmt`).
   - Keep pull requests focused on a single change or feature.
   - Run tests to ensure everything passes:
     ```bash
     cargo test --workspace
     ```
   - Ensure the release binary compiles cleanly:
     ```bash
     cargo build --release
     ```
3. **Commit and push**:
   ```bash
   git commit -m "Add amazing feature"
   git push origin feature/amazing-feature
   ```
4. **Open a Pull Request** against the `main` branch.

## Code of Conduct
Please be respectful and constructive in all discussions, issues, and code reviews. We strive to maintain a welcoming, inclusive, and collaborative environment.
