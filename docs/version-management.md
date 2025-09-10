# Version Management Guide

This guide explains how to properly manage versions for Porter, including updating the brew formula.

## 🎯 **The Problem**

When you install Porter via brew, it shows version `0.1.0` instead of the current version `1.0.3`. This happens because:

1. The brew formula has a hardcoded version
2. The brew formula is not automatically updated when you release new versions
3. The SHA256 hash in the formula doesn't match the new release

## 🔧 **The Solution**

We've created automated tools to keep the brew formula in sync with your releases.

### Files Created

- `porter.rb` - The brew formula file
- `scripts/update-brew-formula.sh` - Script to update the formula
- `scripts/bump-version.sh` - Script to bump versions
- `.github/workflows/update-brew-formula.yml` - GitHub Actions workflow

## 📋 **How to Update Versions**

### Option 1: Using the Bump Script (Recommended)

```bash
# Bump patch version (1.0.3 -> 1.0.4)
./scripts/bump-version.sh patch

# Bump minor version (1.0.3 -> 1.1.0)
./scripts/bump-version.sh minor

# Bump major version (1.0.3 -> 2.0.0)
./scripts/bump-version.sh major
```

### Option 2: Manual Update

1. **Update Cargo.toml**:
   ```toml
   version = "1.0.4"
   ```

2. **Update brew formula**:
   ```bash
   ./scripts/update-brew-formula.sh
   ```

3. **Commit and tag**:
   ```bash
   git add .
   git commit -m "Bump version to 1.0.4"
   git tag v1.0.4
   git push && git push --tags
   ```

## 🚀 **Release Process**

### 1. Bump Version
```bash
./scripts/bump-version.sh patch
```

### 2. Review Changes
```bash
git diff
```

### 3. Commit and Tag
```bash
git add .
git commit -m "Bump version to 1.0.4"
git tag v1.0.4
git push && git push --tags
```

### 4. Create GitHub Release
```bash
# Using GitHub CLI
gh release create v1.0.4

# Or manually via GitHub web interface
```

### 5. Update Homebrew Tap

If you have a separate homebrew tap repository:

```bash
# Clone your homebrew tap
git clone https://github.com/your-username/homebrew-porter.git
cd homebrew-porter

# Copy the updated formula
cp ../porter/porter.rb Formula/porter.rb

# Commit and push
git add Formula/porter.rb
git commit -m "Update porter to v1.0.4"
git push
```

## 🔄 **Automated Updates**

The GitHub Actions workflow (`.github/workflows/update-brew-formula.yml`) will automatically:

1. ✅ Detect new releases
2. ✅ Download the release tarball
3. ✅ Calculate the SHA256 hash
4. ✅ Update the brew formula
5. ✅ Commit the changes

## 🧪 **Testing the Formula**

### Test Locally
```bash
# Install from local formula
brew install --build-from-source ./porter.rb

# Check version
porter --version
```

### Test with Homebrew Tap
```bash
# Add your tap
brew tap your-username/porter

# Install
brew install porter

# Check version
porter --version
```

## 📁 **File Structure**

```
porter/
├── porter.rb                           # Brew formula
├── scripts/
│   ├── bump-version.sh                 # Version bumping script
│   └── update-brew-formula.sh          # Formula update script
├── .github/workflows/
│   └── update-brew-formula.yml         # Automated updates
└── docs/
    └── version-management.md           # This guide
```

## 🐛 **Troubleshooting**

### Version Still Shows 0.1.0

1. **Check if you're using the right tap**:
   ```bash
   brew list porter
   ```

2. **Uninstall and reinstall**:
   ```bash
   brew uninstall porter
   brew install your-username/porter/porter
   ```

3. **Check formula version**:
   ```bash
   brew info porter
   ```

### SHA256 Mismatch

If you get a SHA256 mismatch error:

1. **Regenerate the formula**:
   ```bash
   ./scripts/update-brew-formula.sh
   ```

2. **Check the tarball URL**:
   ```bash
   curl -L https://github.com/umi-labs/porter/archive/v1.0.3.tar.gz | shasum -a 256
   ```

## ✅ **Verification**

After updating, verify everything works:

```bash
# Check local build
./target/debug/porter --version

# Check brew installation
brew install --build-from-source ./porter.rb
porter --version

# Both should show the same version
```

## 🎉 **Success**

Once properly set up, your brew formula will automatically stay in sync with your releases, and users will always get the correct version when they install via brew!
