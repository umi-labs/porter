# Porter

**Porter** is a high-performance, Rust-based CLI tool for migrating structured content between different systems. It supports complex nested data structures, parallel processing, and provides an intuitive interactive interface for data transformation.

## 🚀 Features

- **🔄 Interactive Mapping** - Guided field mapping with arrow key navigation and progress tracking
- **🔗 Nested Data Support** - Handle complex nested structures with dot notation and array operations
- **⚡ High Performance** - Parallel processing with rayon and memory-optimized batch processing
- **📊 Batch Processing** - Efficient processing of large datasets with resume capability
- **✅ Validation** - Comprehensive mapping validation and error reporting
- **🔌 Plugin System** - Extensible adapter system for new source and target formats
- **📝 Configuration Management** - Multi-collection configuration with TOML files

## 📚 Documentation

- **[📖 User Guide](docs/user-guide.md)** - Complete guide for using Porter
- **[🔧 Maintainer Guide](docs/maintainer-guide.md)** - Development and contribution guide
- **[📋 API Reference](docs/api-reference.md)** - Complete API documentation
- **[📊 Tasks & Roadmap](docs/tasks.toml)** - Project progress and future plans
- **[📁 Documentation Overview](docs/README.md)** - All documentation in one place

## Installation

### 🍺 Homebrew (Recommended)

The easiest way to install Porter is via Homebrew:

```bash
brew install umi-labs/tap/porter
```

This will install the latest stable version and keep it updated with `brew upgrade`.

### 📦 Pre-built Binaries

Download pre-built binaries for your platform from the [GitHub releases page](https://github.com/umi-labs/porter/releases):

- **macOS (Apple Silicon)**: `porter-aarch64-apple-darwin.tar.xz`
- **macOS (Intel)**: `porter-x86_64-apple-darwin.tar.xz`
- **Windows**: `porter-x86_64-pc-windows-msvc.zip`
- **Linux (ARM64)**: `porter-aarch64-unknown-linux-gnu.tar.xz`
- **Linux (x64)**: `porter-x86_64-unknown-linux-gnu.tar.xz`

### 🔧 Building from Source

#### Prerequisites
- Rust toolchain (2024 edition)
- Cargo package manager

**Note**: For global installation, ensure Cargo's bin directory is in your PATH:
- **Unix/Linux/macOS**: Usually `~/.cargo/bin/`
- **Windows**: Usually `%USERPROFILE%\.cargo\bin\`

You can check with: `echo $PATH` (Unix) or `echo %PATH%` (Windows)

#### Build Steps
```bash
# Clone the repository
git clone https://github.com/umi-labs/porter.git
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

## 🚀 Quick Start

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

## 📖 Usage

Porter is designed to be run from within your target project repository.

### Basic Usage
```bash
porter --source umbraco --target payload --source-data ./data/umbraco.json --collection hotels
```

### With All Options
```bash
porter \
  --source=umbraco \
  --target=payload \
  --source-data=../migrations/umbraco1.json,../migrations/umbraco2.json \
  --output=./seed \
  --collection=hotels \
  --collection-path=./src/collections/hotels.ts \
  --interactive \
  --verbose
```

### Quick Testing with Fixtures Mode
```bash
porter --fixtures
```

### Configuration File

Porter uses a TOML configuration file (`porter.config.toml`) for multi-collection setups:

```toml
source = "umbraco"
target = "payload"
output = "./seed"
interactive = true

[[collections]]
name = "hotels"
source_data = "./data/hotels.json"
collection_path = "./schemas/hotels.ts"
locale = "en"

[[collections]]
name = "pages"
source_data = "./data/pages.json"
collection_path = "./schemas/pages.ts"
locale = "en"
```

## 🎯 Core Functionality

### Interactive Mapping System
- **Smart Field Detection** - Automatically suggests field mappings based on name similarity
- **Arrow Key Navigation** - Intuitive selection with keyboard navigation
- **Progress Tracking** - Real-time progress bars and statistics
- **Color-coded Interface** - Different colors for different types of information
- **Pagination** - Large option lists are paginated for better usability

### Nested Data Support
- **Dot Notation** - `user.profile.address.city` for nested fields
- **Array Indexing** - `items[0].name` for specific array elements
- **Wildcard Arrays** - `items[*].name` for all array elements
- **Array Slicing** - `items[1:3].price` for array ranges

### Performance Optimization
- **Automatic Strategy Selection**:
  - Small datasets (<100 docs): Simple sequential processing
  - Medium datasets (100-1000 docs): Parallel processing with rayon
  - Large datasets (>1000 docs): Memory-aware batch processing
- **Memory Management** - Adaptive chunk sizing and memory monitoring
- **Resume Capability** - Continue interrupted migrations from checkpoints

### Validation System
- **Schema Validation** - Ensures mappings match target schema requirements
- **Type Compatibility** - Validates data type conversions
- **Required Fields** - Checks that required fields are mapped
- **Transform Validation** - Validates transformation functions

## 🔧 Advanced Features

### Batch Processing
For large datasets (>1000 documents), Porter automatically uses batch processing:
- **Memory Optimization** - Processes data in chunks to manage memory usage
- **Resume Capability** - Can resume interrupted migrations from checkpoints
- **Progress Tracking** - Real-time progress reporting with detailed statistics
- **Error Isolation** - Individual document failures don't stop the entire process

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

## Workflow

1. **Initial Run**: Porter analyzes your source data and target collection schema to generate a mapping file.
2. **Interactive Mapping**: If run with `--interactive`, Porter will prompt you to confirm or modify field mappings.
3. **Transformation**: Porter applies the mapping to transform your source data into the target format.
4. **Output**: Porter generates seed files in the specified output directory.

## Examples

### Migrating Umbraco to Payload CMS

```bash
# Basic migration
porter --source umbraco --target payload --source-data ./data/umbraco.json --collection hotels

# Interactive mapping
porter --source umbraco --target payload --source-data ./data/umbraco.json --collection hotels --interactive

# Using collection schema for better mapping
porter --source umbraco --target payload --source-data ./data/umbraco.json --collection hotels --collection-path ./src/collections/hotels.ts --interactive

# Dry run (no files written)
porter --source umbraco --target payload --source-data ./data/umbraco.json --collection hotels --dry-run
```

### Complex Nested Data Example

```json
{
  "field_mappings": [
    {
      "from": "user.profile.address.city",
      "to": "location"
    },
    {
      "from": "items[0].name",
      "to": "primaryItem"
    },
    {
      "from": "items[*].price",
      "to": "allPrices"
    }
  ]
}
```

## 📊 Project Status

- **Total Tasks**: 20
- **Completed**: 15 (75%)
- **Test Coverage**: 45 comprehensive tests
- **Status**: Major features complete

### Recently Completed

- ✅ **Nested Mappings** - Complex data structure support
- ✅ **Performance Optimization** - Parallel processing with rayon
- ✅ **Batch Processing** - Large dataset handling
- ✅ **Mapping Validation** - Comprehensive validation system
- ✅ **Interactive UI** - Enhanced user experience

## Development

### Adding New Source Adapters
To add support for a new source format, implement the `SourceReader` trait in a new module under `src/sources/`.

### Adding New Target Adapters
To add support for a new target format, implement the `TargetWriter` trait in a new module under `src/targets/`.

### Adding New Transformations
Add new transformation functions to the `src/mapping/transforms.rs` file.

### Testing
```bash
# Run all tests
cargo test

# Run specific test categories
cargo test --lib mapping
cargo test --lib performance

# Run with output
cargo test -- --nocapture
```

## 🤝 Contributing

We welcome contributions! Please see our [Contributing Guidelines](docs/maintainer-guide.md#contributing-guidelines) for details.

### Quick Start for Contributors

1. **Fork the repository**
2. **Create a feature branch**: `git checkout -b feature/amazing-feature`
3. **Make your changes** and add tests
4. **Run tests**: `cargo test`
5. **Submit a pull request**

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## 🆘 Support

- **Documentation**: Start with the [User Guide](docs/user-guide.md)
- **Issues**: [GitHub Issues](https://github.com/umi-labs/porter/issues)
- **Discussions**: [GitHub Discussions](https://github.com/umi-labs/porter/discussions)
- **API Questions**: Check the [API Reference](docs/api-reference.md)

---

**Ready to get started?** Check out the [User Guide](docs/user-guide.md) for detailed instructions!
