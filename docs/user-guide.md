# Porter User Guide

Welcome to Porter! This guide will help you get started with migrating content between different systems.

## Table of Contents

1. [Installation](#installation)
2. [Updating Porter](#updating-porter)
3. [Checking for Updates](#checking-for-updates)
4. [Package Management Best Practices](#package-management-best-practices)
5. [Quick Start](#quick-start)
6. [Configuration](#configuration)
7. [Field Mapping](#field-mapping)
8. [Progress Tracking](#progress-tracking)
9. [Troubleshooting](#troubleshooting)

## Installation

### Homebrew (macOS/Linux)

```bash
# Install Porter
brew install porter

# Update Porter to the latest version
brew update
brew upgrade porter

# Check current version
porter --version
```

### Checking for Updates

#### Homebrew Users

```bash
# Check if updates are available
brew outdated

# Check Porter specifically
brew outdated porter

# See what would be updated
brew upgrade --dry-run porter
```

#### Manual Installation Users

```bash
# Check current version
porter --version

# Check for new releases on GitHub
# Visit: https://github.com/umi-labs/porter/releases

# Or use GitHub CLI
gh repo view umi-labs/porter --json releases
```

#### Development Version

If you're tracking the development branch:

```bash
# Check current commit
git log --oneline -1

# Check for new commits
git fetch origin
git log HEAD..origin/main --oneline

# See what files have changed
git diff HEAD origin/main --name-only
```

## Package Management Best Practices

### Recommended Installation Method

**Homebrew is the recommended installation method** for most users because it:

- Automatically handles dependencies
- Provides easy updates with `brew upgrade`
- Manages PATH configuration
- Offers rollback capabilities
- Integrates with macOS system updates

### When to Use Manual Installation

Consider manual installation if you:

- Need a specific version not available in Homebrew
- Want to contribute to development
- Need to modify the source code
- Are on a platform not supported by Homebrew

### Version Management

#### Stable vs Development

- **Stable releases**: Available via Homebrew, recommended for production use
- **Development builds**: Latest features but may be unstable, use for testing

#### Pinning Versions

If you need to stick to a specific version:

```bash
# Homebrew (not recommended, but possible)
brew install umi-labs/tap/porter@<version>

# Manual installation
git checkout v<version>
cargo install --path .
```

### Backup and Recovery

Before major updates, consider backing up your configuration:

```bash
# Backup your Porter configuration
cp ~/.porter/config.toml ~/.porter/config.toml.backup

# Backup your mappings
cp -r ./mappings ~/porter-mappings-backup
```

### Manual Installation

If you prefer to build from source:

```bash
# Clone the repository
git clone https://github.com/your-org/porter.git
cd porter

# Build and install
cargo build --release
cargo install --path .
```

### Updating Porter

#### Homebrew Users

```bash
# Update Homebrew and Porter
brew update
brew upgrade porter

# Verify the update
porter --version
```

#### Manual Installation Users

```bash
# Navigate to your Porter directory
cd porter

# Pull latest changes
git pull origin main

# Rebuild and reinstall
cargo build --release
cargo install --path . --force
```

#### Development Updates

If you're working with a development version:

```bash
# Pull latest changes
git pull origin main

# Update dependencies
cargo update

# Rebuild
cargo build --release
```

## Troubleshooting

### Update Issues

#### Homebrew Update Problems

If you encounter issues updating via Homebrew:

```bash
# Clean Homebrew cache
brew cleanup

# Update Homebrew itself
brew update

# Try upgrading again
brew upgrade porter

# If still having issues, try uninstalling and reinstalling
brew uninstall porter
brew install umi-labs/tap/porter
```

#### Manual Installation Issues

If you encounter build errors after updating:

```bash
# Clean build artifacts
cargo clean

# Update Rust toolchain
rustup update

# Rebuild
cargo build --release
```

#### Version Conflicts

If you have multiple Porter installations:

```bash
# Check which Porter is being used
which porter

# Check all Porter installations
find /usr/local/bin /opt/homebrew/bin ~/.cargo/bin -name "porter" 2>/dev/null

# Remove old installations if needed
rm /path/to/old/porter
```

### Common Issues

#### "Command not found: porter"

This usually means Porter isn't in your PATH:

```bash
# Check if Porter is installed
brew list | grep porter

# Add Homebrew to PATH (if not already done)
echo 'export PATH="/opt/homebrew/bin:$PATH"' >> ~/.zshrc
source ~/.zshrc
```

#### Permission Errors

If you get permission errors during installation:

```bash
# Fix Homebrew permissions
sudo chown -R $(whoami) /opt/homebrew

# Or for Intel Macs
sudo chown -R $(whoami) /usr/local
```

## Quick Start

### 1. Initialize Configuration

```bash
# Create a new configuration file
porter init

# This will guide you through setting up your first collection
```

### 2. Run Migration

```bash
# Run with interactive mapping
porter --interactive

# Run with existing configuration
porter
```

## Configuration

Porter uses a TOML configuration file (`porter.config.toml`) to define your migration settings.

### Configuration Structure

```toml
# Global settings
source = "umbraco"
target = "payload"
output = "./seed"
interactive = true
verbose = true
dry_run = false

# Plugin directory (optional)
plugin_dir = "./plugins"

# Collections configuration
[[collections]]
name = "hotels"
source_data = "./data/hotels.json"
collection_path = "./schemas/hotels.ts"
locale = "en"
related_collections = ["amenities", "reviews"]

[[collections]]
name = "pages"
source_data = "./data/pages.json"
collection_path = "./schemas/pages.ts"
locale = "en"
```

### Configuration Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `source` | string | - | Source adapter name (e.g., "umbraco") |
| `target` | string | - | Target adapter name (e.g., "payload") |
| `output` | string | `./seed` | Output directory for generated files |
| `interactive` | boolean | `true` | Enable interactive mapping mode |
| `verbose` | boolean | `true` | Enable verbose logging |
| `dry_run` | boolean | `false` | Validate mappings without generating output |
| `plugin_dir` | string | - | Directory containing custom plugins |

### Collection Configuration

Each collection represents a content type or data structure:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Collection name (used for output files) |
| `source_data` | string | Yes | Path to source data file |
| `collection_path` | string | No | Path to TypeScript schema file |
| `locale` | string | No | Locale for internationalized content |
| `related_collections` | array | No | Related collection names |

## Usage

### Basic Commands

```bash
# Show help
porter --help

# Initialize configuration
porter init

# Show current configuration
porter config

# List available adapters
porter --list-adapters

# Run migration with specific options
porter --source=umbraco --target=payload --interactive
```

### Command Line Options

| Option | Description |
|--------|-------------|
| `--source <SOURCE>` | Source adapter name |
| `--target <TARGET>` | Target adapter name |
| `--output <OUTPUT>` | Output directory |
| `--interactive` | Enable interactive mode |
| `--dry-run` | Validate without generating output |
| `--verbose` | Enable verbose logging |
| `--debug` | Enable debug logging |
| `--fixtures` | Use built-in test data |
| `--list-adapters` | List available source and target adapters |

### Interactive Mode

When running in interactive mode, Porter will:

1. **Analyze your data** - Extract field names from source and target schemas
2. **Guide mapping creation** - Present options for each target field
3. **Provide suggestions** - Auto-match fields with similar names
4. **Show progress** - Display real-time progress and statistics
5. **Handle errors gracefully** - Continue processing even if some mappings fail

### Interactive Features

- **Arrow Key Navigation** - Use ↑↓ to select options
- **Color-coded Interface** - Different colors for different types of information
- **Pagination** - Large option lists are paginated for better usability
- **Progress Tracking** - Real-time progress bars and statistics
- **Confirmation Prompts** - Confirm important decisions with default values

## Field Mapping

### Basic Mapping

For each target field, you'll be prompted to select a source field. The system will:

1. Show available source fields
2. Allow you to select a field or skip
3. Ask if transformations are needed
4. Apply the mapping

### Transformations

Porter supports several transformation types:

#### Coordinate Transformations

**Convert to Point (for coordinates)**
- Use when source has a single coordinate object: `{ lat: 1.23, lng: 4.56 }`
- Automatically extracts `lat` and `lng` properties
- Converts to GeoJSON Point format: `{ "type": "Point", "coordinates": [lng, lat] }`

**Combine Coordinates from Separate Fields**
- Use when source has separate latitude and longitude fields
- Examples: `latitude: 1.23` and `longitude: 4.56`
- **Important**: When using this option, select either the latitude or longitude field as the source field - the transform will automatically find both fields in the document
- Smart field selection with coordinate-related keywords prioritized
- Combines into GeoJSON Point format

**Field Selection Tips for Coordinate Combination:**
- Choose either the latitude or longitude field as your source field
- The system will automatically detect and use both fields
- Coordinate-related fields (containing "lat", "latitude", "lng", "longitude", "x", "y") are prioritized in the selection list

#### Other Transformations

**Split by Comma**
- Splits comma-separated strings into arrays
- Example: `"tag1, tag2, tag3"` → `["tag1", "tag2", "tag3"]`

**Custom Transformations**
- Define custom JSON transformations
- Advanced users can specify complex transformation logic

## Mapping System

Porter supports both simple and complex field mappings.

### Simple Mappings

Map a source field directly to a target field:

```json
{
  "from": "title",
  "to": "title"
}
```

### Nested Mappings

Map nested or array fields using dot notation:

```json
{
  "from": "user.profile.name",
  "to": "author"
}
```

### Array Mappings

Access array elements and properties:

```json
{
  "from": "items[0].name",
  "to": "primaryItem"
}
```

### Wildcard Mappings

Process all elements in an array:

```json
{
  "from": "items[*].name",
  "to": "allItemNames"
}
```

### Slice Mappings

Access a range of array elements:

```json
{
  "from": "items[1:3].price",
  "to": "priceRange"
}
```

### Transformations

Apply transformations to field values:

```json
{
  "from": "title",
  "to": "title",
  "transforms": ["uppercase", "trim"]
}
```

### Fallback Values

Provide default values for missing fields:

```json
{
  "from": "description",
  "to": "description",
  "fallback": "No description available"
}
```

## Advanced Features

### Batch Processing

For large datasets (>1000 documents), Porter automatically uses batch processing:

- **Memory Optimization** - Processes data in chunks to manage memory usage
- **Resume Capability** - Can resume interrupted migrations from checkpoints
- **Progress Tracking** - Real-time progress reporting with detailed statistics
- **Error Isolation** - Individual document failures don't stop the entire process

### Parallel Processing

Porter automatically optimizes processing based on dataset size:

- **Small datasets** (<100 docs): Simple sequential processing
- **Medium datasets** (100-1000 docs): Parallel processing with rayon
- **Large datasets** (>1000 docs): Memory-aware batch processing

### Validation System

Porter includes comprehensive validation:

- **Schema Validation** - Ensures mappings match target schema requirements
- **Type Compatibility** - Validates data type conversions
- **Required Fields** - Checks that required fields are mapped
- **Transform Validation** - Validates transformation functions

### Dry Run Mode

Test your configuration without generating output:

```bash
porter --dry-run
```

This will:
- Validate all mappings
- Check data integrity
- Report any issues
- Show what would be generated

## Examples

### Example 1: Simple Blog Migration

**Source Data (WordPress):**
```json
{
  "posts": [
    {
      "ID": 1,
      "post_title": "Hello World",
      "post_content": "This is my first post",
      "post_date": "2023-01-01 10:00:00"
    }
  ]
}
```

**Target Schema (Payload):**
```typescript
export const Posts: CollectionConfig = {
  slug: 'posts',
  fields: [
    {
      name: 'title',
      type: 'text',
      required: true
    },
    {
      name: 'content',
      type: 'richText'
    },
    {
      name: 'publishedDate',
      type: 'date'
    }
  ]
}
```

**Mapping Configuration:**
```json
{
  "field_mappings": [
    {
      "from": "post_title",
      "to": "title"
    },
    {
      "from": "post_content",
      "to": "content"
    },
    {
      "from": "post_date",
      "to": "publishedDate",
      "transforms": ["parseDate"]
    }
  ]
}
```

### Example 2: Complex E-commerce Migration

**Source Data (Umbraco):**
```json
{
  "products": [
    {
      "id": 1,
      "name": "Premium Widget",
      "variants": [
        {
          "sku": "WID-001",
          "price": 29.99,
          "stock": 100
        }
      ],
      "categories": ["electronics", "gadgets"]
    }
  ]
}
```

**Nested Mapping:**
```json
{
  "field_mappings": [
    {
      "from": "name",
      "to": "title"
    },
    {
      "from": "variants[0].sku",
      "to": "primarySku"
    },
    {
      "from": "variants[*].price",
      "to": "prices"
    },
    {
      "from": "categories",
      "to": "tags",
      "transforms": ["join", ","]
    }
  ]
}
```

## Troubleshooting

### Common Issues

#### 1. "porter: command not found" (after installation)

**Solution:** 
- Ensure Cargo's bin directory is in your PATH
- **Unix/Linux/macOS**: Add `export PATH="$HOME/.cargo/bin:$PATH"` to your shell profile
- **Windows**: Add `%USERPROFILE%\.cargo\bin\` to your PATH environment variable
- Restart your terminal after making PATH changes

#### 2. "No configuration file found"

#### 3. "Unsupported source/target"

**Solution:** Check available adapters with `porter --list-adapters`.

#### 4. "Field mapping failed"

**Solution:** 
- Check that source field exists in your data
- Verify target field exists in your schema
- Use `--dry-run` to validate mappings

#### 5. "Memory usage high"

**Solution:**
- Porter automatically handles this with batch processing
- For very large datasets, consider splitting into multiple collections

#### 6. "Interactive mode not working"

**Solution:**
- Ensure `interactive = true` in your configuration
- Check that you're not using `--dry-run` mode

### Performance Tips

1. **Use batch processing** for datasets >1000 documents
2. **Enable parallel processing** for medium-sized datasets
3. **Use dry-run mode** to validate before full migration
4. **Split large datasets** into multiple collections if needed

### Getting Help

- **Documentation**: Check the [maintainer guide](maintainer-guide.md)
- **Issues**: Report bugs on the [GitHub issues page](https://github.com/your-org/porter/issues)
- **Discussions**: Join the [community discussions](https://github.com/your-org/porter/discussions)

## Next Steps

- Read the [maintainer guide](maintainer-guide.md) for development information
- Check out [examples](examples/) for more use cases
- Explore [plugins](plugins/) for extending functionality

## Progress Tracking

Porter provides comprehensive progress tracking at multiple levels:

### Collection-Level Progress

When processing multiple collections, Porter shows:

- **Current Collection**: `🔄 Processing Collection: 1/3 - hotels`
- **Collections Remaining**: `📋 Collections remaining: 2`
- **Collection Progress**: Clear indication of which collection is being processed

### Field-Level Progress

During interactive field mapping, Porter displays:

- **Collection Context**: `=== Collection: hotels ===`
- **Collection Progress**: `=== Collection Progress: 1/5 ===`
- **Field Progress**: `=== Field Progress: 1/40 ===`
- **Current Field**: `=== Mapping for target field: title ===`
- **Progress Summary**: `📊 Progress Summary - Collection: hotels (1/5)`

### Progress Components

- **Progress Bars**: Visual progress indicators with percentages
- **Completed Mappings**: List of successfully mapped fields
- **Remaining Fields**: Preview of upcoming fields to map
- **Color Coding**: Different colors for different types of information
- **Dual Progress Tracking**: Both collection-level and field-level progress

## Quick Reference

### Common Update Commands

| Action | Homebrew | Manual Installation |
|--------|----------|-------------------|
| Check version | `porter --version` | `porter --version` |
| Update | `brew upgrade porter` | `git pull && cargo install --path . --force` |
| Check for updates | `brew outdated porter` | `git fetch && git log HEAD..origin/main` |
| Reinstall | `brew reinstall porter` | `cargo clean && cargo install --path .` |
| Uninstall | `brew uninstall porter` | `cargo uninstall porter` |

### Version Information

```bash
# Get detailed version info
porter --version

# Check build information
porter --help

# Verify installation
which porter
```

### Configuration Backup

```bash
# Backup before updates
cp ~/.porter/config.toml ~/.porter/config.toml.backup
cp -r ./mappings ~/porter-mappings-backup

# Restore if needed
cp ~/.porter/config.toml.backup ~/.porter/config.toml
cp -r ~/porter-mappings-backup ./mappings
```
