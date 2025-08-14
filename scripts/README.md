# Porter Release Scripts

This directory contains automation scripts for managing Porter releases.

## 📁 Scripts Overview

### `release.sh` - Complete Release Automation
The main release script that handles the entire release process from version bumping to pushing tags.

**Usage:**
```bash
# Make executable
chmod +x scripts/release.sh

# Patch release
./scripts/release.sh --patch

# Minor release
./scripts/release.sh --minor

# Major release
./scripts/release.sh --major

# Specific version
./scripts/release.sh 1.0.1

# Dry run
./scripts/release.sh --patch --dry-run
```

**Features:**
- ✅ Automatic version bumping
- ✅ Prerequisites checking
- ✅ Test execution
- ✅ Build verification
- ✅ Git operations (commit, tag, push)
- ✅ Colored output and progress tracking
- ✅ Dry run mode for testing

### `gh-release.sh` - GitHub CLI Release Script
Creates GitHub releases directly using GitHub CLI.

**Usage:**
```bash
# Make executable
chmod +x scripts/gh-release.sh

# Create release
./scripts/gh-release.sh --patch

# Create draft release
./scripts/gh-release.sh --minor --draft

# Custom release notes
./scripts/gh-release.sh 1.0.1 --notes "Bug fixes and performance improvements"
```

**Features:**
- ✅ Direct GitHub release creation
- ✅ Draft and prerelease support
- ✅ Custom release notes
- ✅ GitHub CLI integration

## 🚀 Quick Start

1. **Choose your automation method:**
   - **Local script**: `./scripts/release.sh --patch`
   - **GitHub Actions**: Use the web interface
   - **GitHub CLI**: `./scripts/gh-release.sh --patch`

2. **For first-time setup:**
   ```bash
   # Make scripts executable
   chmod +x scripts/*.sh
   
   # Test with dry run
   ./scripts/release.sh --patch --dry-run
   ```

3. **For regular releases:**
   ```bash
   # Simple patch release
   ./scripts/release.sh --patch
   ```

## 📋 Prerequisites

### For Local Scripts
- ✅ Git repository access
- ✅ Rust development environment
- ✅ Write access to repository

### For GitHub CLI Script
- ✅ GitHub CLI installed (`brew install gh`)
- ✅ Authenticated with GitHub (`gh auth login`)
- ✅ Repository access

## 🔧 Troubleshooting

### Common Issues

**Script not executable:**
```bash
chmod +x scripts/release.sh
```

**GitHub CLI not authenticated:**
```bash
gh auth login
```

**Version conflicts:**
```bash
# Check existing tags
git tag --list

# Remove conflicting tag
git tag -d v1.0.1
git push origin :refs/tags/v1.0.1
```

### Getting Help

- **Script help**: `./scripts/release.sh --help`
- **GitHub CLI help**: `./scripts/gh-release.sh --help`
- **Documentation**: See [maintainer guide](../docs/maintainer-guide.md)

## 📚 Related Documentation

- [Complete Release Walkthrough](../docs/maintainer-guide.md#complete-release-walkthrough)
- [Release Automation](../docs/maintainer-guide.md#-release-automation)
- [Troubleshooting](../docs/maintainer-guide.md#troubleshooting-common-issues)
