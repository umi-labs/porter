#!/bin/bash

# Porter Release Automation Script
# This script automates the release process for Porter

set -e  # Exit on any error

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Function to print colored output
print_status() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

print_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Function to show usage
show_usage() {
    echo "Usage: $0 [OPTIONS] <version>"
    echo ""
    echo "Options:"
    echo "  -h, --help          Show this help message"
    echo "  -p, --patch         Bump patch version (e.g., 1.0.0 -> 1.0.1)"
    echo "  -m, --minor         Bump minor version (e.g., 1.0.0 -> 1.1.0)"
    echo "  -M, --major         Bump major version (e.g., 1.0.0 -> 2.0.0)"
    echo "  -d, --dry-run       Show what would be done without executing"
    echo "  -y, --yes           Skip confirmation prompts"
    echo ""
    echo "Examples:"
    echo "  $0 1.0.1            Release specific version"
    echo "  $0 --patch          Bump patch version automatically"
    echo "  $0 --minor          Bump minor version automatically"
    echo "  $0 --major          Bump major version automatically"
    echo ""
}

# Function to get current version from Cargo.toml
get_current_version() {
    grep '^version = ' Cargo.toml | sed 's/version = "\(.*\)"/\1/'
}

# Function to bump version
bump_version() {
    local current_version=$1
    local bump_type=$2
    
    IFS='.' read -ra VERSION_PARTS <<< "$current_version"
    local major=${VERSION_PARTS[0]}
    local minor=${VERSION_PARTS[1]}
    local patch=${VERSION_PARTS[2]}
    
    case $bump_type in
        "patch")
            patch=$((patch + 1))
            ;;
        "minor")
            minor=$((minor + 1))
            patch=0
            ;;
        "major")
            major=$((major + 1))
            minor=0
            patch=0
            ;;
    esac
    
    echo "$major.$minor.$patch"
}

# Function to update version in Cargo.toml
update_version() {
    local new_version=$1
    local dry_run=$2
    
    if [ "$dry_run" = "true" ]; then
        print_status "Would update Cargo.toml version to $new_version"
        return
    fi
    
    # Update version in Cargo.toml
    if [[ "$OSTYPE" == "darwin"* ]]; then
        # macOS
        sed -i '' "s/^version = \".*\"/version = \"$new_version\"/" Cargo.toml
    else
        # Linux
        sed -i "s/^version = \".*\"/version = \"$new_version\"/" Cargo.toml
    fi
    
    print_success "Updated Cargo.toml version to $new_version"
}

# Function to run tests
run_tests() {
    local dry_run=$1
    
    if [ "$dry_run" = "true" ]; then
        print_status "Would run: cargo test"
        return
    fi
    
    print_status "Running tests..."
    cargo test
    print_success "All tests passed!"
}

# Function to build the project
build_project() {
    local dry_run=$1
    
    if [ "$dry_run" = "true" ]; then
        print_status "Would run: cargo build --release"
        return
    fi
    
    print_status "Building project..."
    cargo build --release
    print_success "Build completed successfully!"
}

# Function to commit changes
commit_changes() {
    local version=$1
    local dry_run=$2
    
    if [ "$dry_run" = "true" ]; then
        print_status "Would commit changes with message: release: bump version to $version"
        return
    fi
    
    git add Cargo.toml Cargo.lock
    git commit -m "release: bump version to $version"
    print_success "Committed version bump"
}

# Function to create and push tag
create_tag() {
    local version=$1
    local dry_run=$2
    
    if [ "$dry_run" = "true" ]; then
        print_status "Would create tag: v$version"
        print_status "Would push tag to origin"
        return
    fi
    
    print_status "Creating tag v$version..."
    git tag -a "v$version" -m "Release v$version"
    print_success "Created tag v$version"
    
    print_status "Pushing tag to origin..."
    git push origin "v$version"
    print_success "Pushed tag v$version to origin"
}

# Function to push changes
push_changes() {
    local dry_run=$1
    
    if [ "$dry_run" = "true" ]; then
        print_status "Would push changes to origin/prod"
        return
    fi
    
    print_status "Pushing changes to origin/prod..."
    git push origin prod
    print_success "Pushed changes to origin/prod"
}

# Function to check prerequisites
check_prerequisites() {
    print_status "Checking prerequisites..."
    
    # Check if we're in a git repository
    if ! git rev-parse --git-dir > /dev/null 2>&1; then
        print_error "Not in a git repository"
        exit 1
    fi
    
    # Check if we're on the prod branch
    local current_branch=$(git branch --show-current)
    if [ "$current_branch" != "prod" ]; then
        print_warning "Not on prod branch (currently on $current_branch)"
        read -p "Continue anyway? (y/N): " -n 1 -r
        echo
        if [[ ! $REPLY =~ ^[Yy]$ ]]; then
            exit 1
        fi
    fi
    
    # Check if working directory is clean
    if ! git diff-index --quiet HEAD --; then
        print_error "Working directory is not clean. Please commit or stash changes first."
        exit 1
    fi
    
    # Check if Cargo.toml exists
    if [ ! -f "Cargo.toml" ]; then
        print_error "Cargo.toml not found in current directory"
        exit 1
    fi
    
    print_success "Prerequisites check passed!"
}

# Function to show release summary
show_summary() {
    local version=$1
    local dry_run=$2
    
    echo ""
    echo "=========================================="
    echo "           RELEASE SUMMARY"
    echo "=========================================="
    echo "Version: $version"
    echo "Tag: v$version"
    echo "Branch: prod"
    echo "Mode: $([ "$dry_run" = "true" ] && echo "DRY RUN" || echo "LIVE")"
    echo ""
    
    if [ "$dry_run" = "false" ]; then
        echo "Next steps:"
        echo "1. Monitor GitHub Actions: https://github.com/umi-labs/porter/actions"
        echo "2. Check release: https://github.com/umi-labs/porter/releases"
        echo "3. Test Homebrew: brew install umi-labs/tap/porter"
        echo ""
    fi
}

# Main script logic
main() {
    local version=""
    local bump_type=""
    local dry_run=false
    local skip_confirmation=false
    
    # Parse command line arguments
    while [[ $# -gt 0 ]]; do
        case $1 in
            -h|--help)
                show_usage
                exit 0
                ;;
            -p|--patch)
                bump_type="patch"
                shift
                ;;
            -m|--minor)
                bump_type="minor"
                shift
                ;;
            -M|--major)
                bump_type="major"
                shift
                ;;
            -d|--dry-run)
                dry_run=true
                shift
                ;;
            -y|--yes)
                skip_confirmation=true
                shift
                ;;
            -*)
                print_error "Unknown option: $1"
                show_usage
                exit 1
                ;;
            *)
                if [ -z "$version" ]; then
                    version=$1
                else
                    print_error "Multiple versions specified"
                    exit 1
                fi
                shift
                ;;
        esac
    done
    
    # Check if version is specified or bump type is provided
    if [ -z "$version" ] && [ -z "$bump_type" ]; then
        print_error "Please specify a version or use --patch, --minor, or --major"
        show_usage
        exit 1
    fi
    
    # If bump type is specified, calculate new version
    if [ -n "$bump_type" ]; then
        local current_version=$(get_current_version)
        version=$(bump_version "$current_version" "$bump_type")
        print_status "Current version: $current_version"
        print_status "New version: $version"
    fi
    
    # Validate version format
    if ! [[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        print_error "Invalid version format: $version. Expected format: X.Y.Z"
        exit 1
    fi
    
    # Check prerequisites
    check_prerequisites
    
    # Show what will be done
    show_summary "$version" "$dry_run"
    
    # Ask for confirmation unless --yes is specified
    if [ "$skip_confirmation" = "false" ] && [ "$dry_run" = "false" ]; then
        read -p "Proceed with release? (y/N): " -n 1 -r
        echo
        if [[ ! $REPLY =~ ^[Yy]$ ]]; then
            print_status "Release cancelled"
            exit 0
        fi
    fi
    
    # Execute release steps
    print_status "Starting release process..."
    
    # Update version
    update_version "$version" "$dry_run"
    
    # Run tests
    run_tests "$dry_run"
    
    # Build project
    build_project "$dry_run"
    
    # Commit changes
    commit_changes "$version" "$dry_run"
    
    # Create and push tag
    create_tag "$version" "$dry_run"
    
    # Push changes
    push_changes "$dry_run"
    
    if [ "$dry_run" = "true" ]; then
        print_success "Dry run completed successfully!"
    else
        print_success "Release process completed successfully!"
        echo ""
        print_status "GitHub Actions will now build and publish the release automatically."
        print_status "Monitor progress at: https://github.com/umi-labs/porter/actions"
    fi
}

# Run main function with all arguments
main "$@"
