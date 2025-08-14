# Porter Maintainer Guide

This guide is for developers contributing to the Porter data migration tool. It covers the codebase architecture, development setup, testing strategies, and contribution guidelines.

## Table of Contents

1. [Architecture Overview](#architecture-overview)
2. [Development Setup](#development-setup)
3. [Code Structure](#code-structure)
4. [Testing Strategy](#testing-strategy)
5. [Adding New Features](#adding-new-features)
6. [Performance Considerations](#performance-considerations)
7. [Contributing Guidelines](#contributing-guidelines)
8. [Release Process](#release-process)

## Architecture Overview

Porter follows a modular, plugin-based architecture designed for extensibility and maintainability.

### Core Components

```
porter/
├── src/
│   ├── main.rs              # CLI entry point
│   ├── lib.rs               # Library exports
│   ├── cli.rs               # Command-line interface
│   ├── config.rs            # Configuration management
│   ├── adapter.rs           # Adapter traits
│   ├── plugin.rs            # Plugin system
│   ├── mapping/             # Mapping system
│   │   ├── mod.rs           # Main mapping logic
│   │   ├── json_mapping.rs  # JSON mapping structures
│   │   ├── nested.rs        # Nested mapping support
│   │   ├── transforms.rs    # Data transformations
│   │   ├── typescript.rs    # TypeScript parsing
│   │   └── validation.rs    # Mapping validation
│   ├── sources/             # Source adapters
│   │   ├── mod.rs
│   │   └── umbraco.rs       # Umbraco source adapter
│   ├── targets/             # Target adapters
│   │   ├── mod.rs
│   │   └── payload.rs       # Payload target adapter
│   ├── batch/               # Batch processing
│   │   └── mod.rs           # Batch processing logic
│   ├── performance/         # Performance optimizations
│   │   └── mod.rs           # Parallel processing
│   └── util/                # Utilities
│       ├── mod.rs
│       ├── fs.rs            # File system utilities
│       └── interact.rs      # Interactive prompts
```

### Key Design Principles

1. **Modularity** - Each component is self-contained with clear interfaces
2. **Extensibility** - Plugin system allows adding new adapters
3. **Performance** - Parallel processing and memory optimization
4. **User Experience** - Interactive mode with progress tracking
5. **Error Handling** - Comprehensive error handling and recovery

## Development Setup

### Prerequisites

- Rust 1.70+
- Git
- A code editor (Rust Rover recommended)

### Local Development

```bash
# Clone the repository
git clone https://github.com/umi-labs/porter.git
cd porter

# Install dependencies
cargo build

# Run tests
cargo test

# Run with debug logging
RUST_LOG=debug cargo run -- --help
```

### Development Tools

#### Recommended VS Code Extensions

- `rust-analyzer` - Rust language support
- `crates` - Cargo.toml dependency management
- `CodeLLDB` - Debugging support

#### Useful Cargo Commands

```bash
# Check for issues without building
cargo check

# Format code
cargo fmt

# Lint code
cargo clippy

# Run specific tests
cargo test --lib mapping
cargo test --lib performance

# Build with optimizations
cargo build --release

# Generate documentation
cargo doc --open
```

## Code Structure

### Module Organization

#### Core Modules

- **`main.rs`** - CLI entry point and argument parsing
- **`lib.rs`** - Library exports and module organization
- **`cli.rs`** - Command-line interface and subcommands
- **`config.rs`** - Configuration management and validation

#### Adapter System

- **`adapter.rs`** - Defines `SourceReader` and `TargetWriter` traits
- **`plugin.rs`** - Plugin loading and management
- **`sources/`** - Source adapter implementations
- **`targets/`** - Target adapter implementations

#### Mapping System

- **`mapping/mod.rs`** - Main mapping orchestration
- **`mapping/json_mapping.rs`** - Mapping data structures
- **`mapping/nested.rs`** - Nested field mapping support
- **`mapping/transforms.rs`** - Data transformation functions
- **`mapping/typescript.rs`** - TypeScript schema parsing
- **`mapping/validation.rs`** - Mapping validation logic

#### Performance Modules

- **`batch/mod.rs`** - Batch processing for large datasets
- **`performance/mod.rs`** - Parallel processing with rayon

#### Utilities

- **`util/fs.rs`** - File system operations
- **`util/interact.rs`** - Interactive user prompts

### Key Data Structures

#### Configuration

```rust
pub struct PorterConfig {
    pub source: String,
    pub target: String,
    pub output: String,
    pub collections: Vec<CollectionConfig>,
    pub interactive: bool,
    pub verbose: bool,
    pub dry_run: bool,
    pub plugin_dir: Option<String>,
}

pub struct CollectionConfig {
    pub name: String,
    pub source_data: String,
    pub collection_path: Option<String>,
    pub locale: Option<String>,
    pub related_collections: Option<Vec<String>>,
}
```

#### Mapping

```rust
pub struct Mapping {
    pub source: String,
    pub target: String,
    pub collection: String,
    pub field_mappings: Vec<FieldMapping>,
    pub block_mappings: Option<Vec<BlockMapping>>,
}

pub struct FieldMapping {
    pub to: String,
    pub from: Value,
    pub transforms: Vec<String>,
    pub fallback: Option<String>,
}
```

#### Nested Mapping

```rust
pub struct NestedPath {
    pub segments: Vec<PathSegment>,
}

pub enum PathSegment {
    Field(String),
    Index(usize),
    Wildcard,
    Slice(Option<usize>, Option<usize>),
}
```

## Testing Strategy

### Test Organization

Tests are organized by module and functionality:

```
tests/
├── unit/                    # Unit tests (in module files)
├── integration/             # Integration tests
│   └── umbraco_to_payload_test.rs
└── fixtures/                # Test data
    ├── umbraco.json
    └── payload-collection.ts
```

### Running Tests

```bash
# Run all tests
cargo test

# Run specific test categories
cargo test --lib mapping
cargo test --lib performance
cargo test --lib config

# Run with output
cargo test -- --nocapture

# Run integration tests
cargo test --test umbraco_to_payload_test
```

### Test Coverage

Current test coverage includes:

- **Configuration Management** - 9 tests
- **Mapping System** - 10 tests
- **Nested Mapping** - 10 tests
- **Batch Processing** - 4 tests
- **Performance Optimization** - 5 tests
- **Validation System** - 4 tests
- **Integration Tests** - 1 test

**Total: 45 tests**

### Writing Tests

#### Unit Test Example

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_field_mapping_creation() {
        let mapping = FieldMapping {
            to: "title".to_string(),
            from: json!("nodeName"),
            transforms: Vec::new(),
            fallback: None,
        };

        assert_eq!(mapping.to, "title");
        assert_eq!(mapping.from, json!("nodeName"));
    }
}
```

#### Integration Test Example

```rust
#[test]
fn test_umbraco_to_payload_migration() {
    let source = UmbracoSource::new();
    let target = PayloadTarget::new();
    
    // Test data and assertions
    let result = perform_migration(source, target, test_data);
    assert!(result.is_ok());
}
```

## Adding New Features

### Adding a New Source Adapter

1. **Create the adapter module:**

```rust
// src/sources/wordpress.rs
use crate::adapter::SourceReader;
use anyhow::Result;
use serde_json::Value;

pub struct WordPressSource;

impl WordPressSource {
    pub fn new() -> Self {
        Self
    }
}

impl SourceReader for WordPressSource {
    fn read_documents(&self, source_files: &[String]) -> Result<Vec<Value>> {
        // Implementation
    }
}
```

2. **Register the adapter:**

```rust
// src/sources/mod.rs
pub mod wordpress;

// In plugin registration
plugin_manager.register_source("wordpress", Box::new(WordPressSource::new()));
```

3. **Add tests:**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wordpress_source_creation() {
        let source = WordPressSource::new();
        assert!(source.read_documents(&["test.json"]).is_ok());
    }
}
```

### Adding a New Transformation

1. **Add to transforms module:**

```rust
// src/mapping/transforms.rs
pub fn slugify(value: &str) -> Result<String> {
    Ok(value
        .to_lowercase()
        .replace(" ", "-")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .collect())
}
```

2. **Register the transform:**

```rust
pub fn get_transform(name: &str) -> Option<TransformFn> {
    match name {
        "slugify" => Some(slugify),
        // ... other transforms
    }
}
```

3. **Add tests:**

```rust
#[test]
fn test_slugify_transform() {
    let result = slugify("Hello World!").unwrap();
    assert_eq!(result, "hello-world");
}
```

### Adding Performance Optimizations

1. **Profile the code:**

```bash
# Install profiling tools
cargo install flamegraph

# Generate flamegraph
cargo flamegraph --bin porter -- --source=umbraco --target=payload
```

2. **Optimize bottlenecks:**

```rust
// Use parallel processing for expensive operations
use rayon::prelude::*;

documents.par_iter()
    .map(|doc| process_document(doc))
    .collect()
```

3. **Add benchmarks:**

```rust
#[cfg(test)]
mod benches {
    use criterion::{black_box, criterion_group, criterion_main, Criterion};

    fn bench_mapping_application(c: &mut Criterion) {
        c.bench_function("apply_mapping", |b| {
            b.iter(|| apply_mapping(black_box(&test_doc), black_box(&mapping)))
        });
    }

    criterion_group!(benches, bench_mapping_application);
    criterion_main!(benches);
}
```

## Performance Considerations

### Memory Management

- **Batch Processing** - Process large datasets in chunks
- **Streaming** - Use iterators for memory-efficient processing
- **Garbage Collection** - Minimize allocations in hot paths

### Parallel Processing

- **Rayon Integration** - Use parallel iterators for CPU-intensive tasks
- **Thread Pool Management** - Configure appropriate thread counts
- **Load Balancing** - Distribute work evenly across threads

### Optimization Techniques

```rust
// Use references to avoid cloning
fn process_documents(docs: &[Value]) -> Result<Vec<Value>> {
    docs.iter().map(|doc| process_document(doc)).collect()
}

// Use efficient data structures
use std::collections::HashMap;
let field_map: HashMap<String, String> = HashMap::new();

// Avoid unnecessary allocations
let result = format!("processed_{}", doc_id);
```

### Performance Monitoring

```rust
use std::time::Instant;

let start = Instant::now();
let result = process_documents(&documents);
let duration = start.elapsed();

log::info!("Processed {} documents in {:?}", documents.len(), duration);
```

## Contributing Guidelines

### Code Style

- Follow Rust conventions and use `cargo fmt`
- Use meaningful variable and function names
- Add comprehensive documentation
- Include unit tests for new functionality

### Commit Messages

Use conventional commit format:

```
feat: add WordPress source adapter
fix: resolve memory leak in batch processing
docs: update user guide with new examples
test: add integration tests for nested mappings
```

### Pull Request Process

1. **Create a feature branch:**
   ```bash
   git checkout -b feature/wordpress-adapter
   ```

2. **Make your changes:**
   - Add new functionality
   - Write tests
   - Update documentation

3. **Run checks:**
   ```bash
   cargo check
   cargo test
   cargo clippy
   cargo fmt
   ```

4. **Submit PR:**
   - Clear description of changes
   - Link to related issues
   - Include test results

### Review Checklist

- [ ] Code follows Rust conventions
- [ ] Tests pass and cover new functionality
- [ ] Documentation is updated
- [ ] Performance impact is considered
- [ ] Error handling is comprehensive
- [ ] Backward compatibility is maintained

## Release Process

### Version Management

Porter uses semantic versioning (MAJOR.MINOR.PATCH):

- **MAJOR** - Breaking changes
- **MINOR** - New features, backward compatible
- **PATCH** - Bug fixes, backward compatible

### Release Steps

1. **Update version in Cargo.toml:**
   ```toml
   [package]
   version = "0.2.0"
   ```

2. **Update CHANGELOG.md:**
   ```markdown
   ## [0.2.0] - 2024-01-15
   
   ### Added
   - WordPress source adapter
   - Nested mapping support
   - Performance optimizations
   
   ### Changed
   - Improved error messages
   
   ### Fixed
   - Memory leak in batch processing
   ```

3. **Create release tag:**
   ```bash
   git tag -a v0.2.0 -m "Release v0.2.0"
   git push origin v0.2.0
   ```

4. **Build and publish:**
   ```bash
   cargo build --release
   # Upload to GitHub releases
   ```

### GitHub Releases & Homebrew Tap

#### GitHub Releases
- Pre-built binaries are automatically generated for multiple platforms
- Release notes are automatically generated from the changelog
- Assets include checksums for verification

#### Homebrew Tap Setup
The Homebrew tap is configured to automatically update when new releases are published:

```bash
# Users can install via:
brew install umi-labs/tap/porter

# The tap automatically tracks the latest stable release
```

#### Release Assets
Each release includes:
- **macOS (Apple Silicon)**: `porter-aarch64-apple-darwin.tar.xz`
- **macOS (Intel)**: `porter-x86_64-apple-darwin.tar.xz`
- **Windows**: `porter-x86_64-pc-windows-msvc.zip`
- **Linux (ARM64)**: `porter-aarch64-unknown-linux-gnu.tar.xz`
- **Linux (x64)**: `porter-x86_64-unknown-linux-gnu.tar.xz`

### Pre-release Testing

- [ ] All tests pass
- [ ] Documentation is current
- [ ] Performance benchmarks are acceptable
- [ ] Integration tests pass
- [ ] User guide examples work
- [ ] Homebrew installation works correctly
- [ ] Pre-built binaries are functional on all platforms

## Getting Help

- **Code Issues** - Check existing issues and discussions
- **Architecture Questions** - Review this guide and code comments
- **Performance Issues** - Use profiling tools and benchmarks
- **Testing Questions** - Review existing test patterns

## Next Steps

- Review the [user guide](user-guide.md) for user-facing features
- Check [examples](examples/) for usage patterns
- Explore [plugins](plugins/) for extension patterns
- Join the [community discussions](https://github.com/umi-labs/porter/discussions)
