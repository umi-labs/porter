#!/bin/bash

# GitHub CLI Release Script for Porter
# This script uses GitHub CLI to create releases

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

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

show_usage() {
    echo "Usage: $0 [OPTIONS] <version>"
    echo ""
    echo "Options:"
    echo "  -h, --help          Show this help message"
    echo "  -p, --patch         Bump patch version"
    echo "  -m, --minor         Bump minor version"
    echo "  -M, --major         Bump major version"
    echo "  -d, --draft         Create as draft release"
    echo "  -pr, --prerelease   Mark as prerelease"
    echo "  -t, --title         Custom release title"
    echo "  -n, --notes         Custom release notes"
    echo "  -y, --yes           Skip confirmation"
    echo ""
    echo "Examples:"
    echo "  $0 1.0.1            Create release for specific version"
    echo "  $0 --patch          Bump patch and create release"
    echo "  $0 --minor --draft  Create draft minor release"
    echo ""
}

get_current_version() {
    grep '^version = ' Cargo.toml | sed 's/version = "\(.*\)"/\1/'
}

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

check_prerequisites() {
    print_status "Checking prerequisites..."
    
    # Check if gh CLI is installed
    if ! command -v gh &> /dev/null; then
        print_error "GitHub CLI (gh) is not installed. Please install it first."
        echo "Installation: https://cli.github.com/"
        exit 1
    fi
    
    # Check if authenticated
    if ! gh auth status &> /dev/null; then
        print_error "Not authenticated with GitHub CLI. Please run 'gh auth login'"
        exit 1
    fi
    
    # Check if we're in a git repository
    if ! git rev-parse --git-dir > /dev/null 2>&1; then
        print_error "Not in a git repository"
        exit 1
    fi
    
    # Check if Cargo.toml exists
    if [ ! -f "Cargo.toml" ]; then
        print_error "Cargo.toml not found in current directory"
        exit 1
    fi
    
    print_success "Prerequisites check passed!"
}

create_release() {
    local version=$1
    local draft=$2
    local prerelease=$3
    local title=$4
    local notes=$5
    
    local release_title="${title:-"Release v$version"}"
    local release_notes="${notes:-"Release v$version of Porter"}"
    
    local gh_args=""
    
    if [ "$draft" = "true" ]; then
        gh_args="$gh_args --draft"
    fi
    
    if [ "$prerelease" = "true" ]; then
        gh_args="$gh_args --prerelease"
    fi
    
    print_status "Creating GitHub release..."
    gh release create "v$version" \
        --title "$release_title" \
        --notes "$release_notes" \
        $gh_args
    
    print_success "GitHub release created successfully!"
}

main() {
    local version=""
    local bump_type=""
    local draft=false
    local prerelease=false
    local title=""
    local notes=""
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
            -d|--draft)
                draft=true
                shift
                ;;
            -pr|--prerelease)
                prerelease=true
                shift
                ;;
            -t|--title)
                title="$2"
                shift 2
                ;;
            -n|--notes)
                notes="$2"
                shift 2
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
    
    # Show release summary
    echo ""
    echo "=========================================="
    echo "           RELEASE SUMMARY"
    echo "=========================================="
    echo "Version: $version"
    echo "Tag: v$version"
    echo "Draft: $draft"
    echo "Prerelease: $prerelease"
    echo "Title: ${title:-"Release v$version"}"
    echo ""
    
    # Ask for confirmation unless --yes is specified
    if [ "$skip_confirmation" = "false" ]; then
        read -p "Proceed with creating GitHub release? (y/N): " -n 1 -r
        echo
        if [[ ! $REPLY =~ ^[Yy]$ ]]; then
            print_status "Release cancelled"
            exit 0
        fi
    fi
    
    # Create the release
    create_release "$version" "$draft" "$prerelease" "$title" "$notes"
    
    print_success "Release process completed!"
    echo ""
    print_status "Release URL: https://github.com/umi-labs/porter/releases/tag/v$version"
}

main "$@"
