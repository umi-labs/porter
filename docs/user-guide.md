# Porter User Guide

Porter is a powerful, high-performance data migration tool designed to transform structured content between different systems. It supports complex nested data structures, parallel processing, and provides an intuitive interactive interface.

## Table of Contents

1. [Installation](#installation)
2. [Quick Start](#quick-start)
3. [Configuration](#configuration)
4. [Usage](#usage)
5. [Mapping System](#mapping-system)
6. [Advanced Features](#advanced-features)
7. [Examples](#examples)
8. [Troubleshooting](#troubleshooting)

## Installation

### Prerequisites

- Rust 1.70+ (for building from source)
- Git (for cloning the repository)

### Building from Source

```bash
# Clone the repository
git clone https://github.com/your-org/porter.git
cd porter

# Build the project
cargo build --release

# Install globally (recommended)
cargo install --path .

# Now you can run 'porter' from anywhere!
porter --help
```

### Alternative: Local Build
```bash
# Build without installing globally
cargo build --release

# Run from the project directory
./target/release/porter --help
```

### Using Pre-built Binaries

Download the latest release from the [releases page](https://github.com/your-org/porter/releases).

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
