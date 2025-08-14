# Porter Documentation

Welcome to the Porter documentation! This directory contains comprehensive documentation for both users and developers of the Porter data migration tool.

## 📚 Documentation Overview

### For Users

- **[User Guide](user-guide.md)** - Complete guide for using Porter
  - Installation and setup
  - Configuration management
  - Interactive mapping
  - Advanced features
  - Troubleshooting

- **[API Reference](api-reference.md)** - Complete API documentation
  - All public functions and structs
  - Code examples
  - Error handling patterns
  - Version compatibility

### For Developers

- **[Maintainer Guide](maintainer-guide.md)** - Development and contribution guide
  - Architecture overview
  - Development setup
  - Testing strategies
  - Adding new features
  - Performance considerations

- **[Tasks & Roadmap](tasks.toml)** - Project tasks and progress tracking
  - Current task status
  - Completed features
  - Future roadmap
  - Progress metrics

## 🚀 Quick Start

### New to Porter?

1. **Read the [User Guide](user-guide.md)** - Start here for installation and basic usage
2. **Try the examples** - Follow the step-by-step examples in the user guide
3. **Check the [API Reference](api-reference.md)** - For programmatic usage

### Contributing to Porter?

1. **Read the [Maintainer Guide](maintainer-guide.md)** - Development setup and guidelines
2. **Review the [Tasks](tasks.toml)** - See what needs to be done
3. **Check the [API Reference](api-reference.md)** - Understand the codebase structure

## 📖 Documentation Structure

```
docs/
├── README.md              # This file - documentation overview
├── user-guide.md          # Complete user guide
├── maintainer-guide.md    # Developer and contributor guide
├── api-reference.md       # Complete API documentation
├── tasks.toml            # Project tasks and roadmap
└── plan.md               # Project planning and architecture
```

## 🎯 Key Features

### For Users

- **Interactive Mapping** - Guided field mapping with arrow key navigation
- **Nested Data Support** - Handle complex nested structures with dot notation
- **Batch Processing** - Efficient processing of large datasets
- **Parallel Processing** - Automatic optimization based on dataset size
- **Validation** - Comprehensive mapping validation and error reporting
- **Resume Capability** - Continue interrupted migrations from checkpoints

### For Developers

- **Modular Architecture** - Clean separation of concerns
- **Plugin System** - Extensible adapter system
- **Comprehensive Testing** - 45+ unit and integration tests
- **Performance Optimized** - Rayon parallel processing and memory management
- **Error Handling** - Robust error handling with detailed reporting
- **Documentation** - Extensive inline documentation and examples

## 🔧 Common Use Cases

### Simple Migration
```bash
# Initialize configuration
porter init

# Run migration
porter --interactive
```

### Complex Nested Data
```json
{
  "from": "user.profile.address.city",
  "to": "location"
}
```

### Array Processing
```json
{
  "from": "items[*].name",
  "to": "allItemNames"
}
```

### Batch Processing
```bash
# Automatic for large datasets (>1000 documents)
porter --source=umbraco --target=payload
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

## 🤝 Getting Help

### Documentation Issues
- Check the relevant guide above
- Review the [API Reference](api-reference.md) for technical details
- Look at the [Tasks](tasks.toml) for known issues

### Code Issues
- Review the [Maintainer Guide](maintainer-guide.md)
- Check existing GitHub issues
- Join community discussions

### Feature Requests
- Review the [Tasks](tasks.toml) for planned features
- Check the [Plan](plan.md) for architectural decisions
- Submit new feature requests via GitHub

## 📝 Contributing to Documentation

### Adding User Documentation

1. **Update [user-guide.md](user-guide.md)** for new features
2. **Add examples** to demonstrate usage
3. **Update troubleshooting** section for common issues
4. **Test all examples** to ensure they work

### Adding Developer Documentation

1. **Update [maintainer-guide.md](maintainer-guide.md)** for new patterns
2. **Update [api-reference.md](api-reference.md)** for new APIs
3. **Add code examples** and usage patterns
4. **Update architecture diagrams** if needed

### Documentation Standards

- **Clear and concise** - Write for the target audience
- **Code examples** - Include working examples
- **Cross-references** - Link between related sections
- **Regular updates** - Keep documentation current with code

## 🔄 Documentation Maintenance

### Regular Reviews

- **Monthly** - Review and update user guide
- **Per Release** - Update API reference
- **Per Feature** - Update maintainer guide
- **Continuous** - Update tasks and progress

### Quality Checklist

- [ ] All examples work and are tested
- [ ] Links are valid and working
- [ ] Code examples are up-to-date
- [ ] Screenshots are current (if applicable)
- [ ] Troubleshooting covers common issues

## 📈 Documentation Metrics

- **User Guide**: ~500 lines, comprehensive coverage
- **Maintainer Guide**: ~600 lines, detailed development info
- **API Reference**: ~800 lines, complete API documentation
- **Tasks**: 20 tasks tracked, 15 completed
- **Test Coverage**: 45 tests documented

## 🎉 Acknowledgments

Thanks to all contributors who have helped improve Porter's documentation:

- **User Feedback** - Shaping the user guide
- **Code Reviews** - Improving maintainer guide
- **Bug Reports** - Enhancing troubleshooting sections
- **Feature Requests** - Expanding use case coverage

---

**Need help?** Start with the [User Guide](user-guide.md) or [Maintainer Guide](maintainer-guide.md) depending on your needs!
