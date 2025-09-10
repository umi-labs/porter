#!/bin/bash

# Script to bump the version in Cargo.toml and update related files
# Usage: ./scripts/bump-version.sh [patch|minor|major]

set -e

# Get current version
CURRENT_VERSION=$(grep '^version = ' Cargo.toml | sed 's/version = "\(.*\)"/\1/')
echo "Current version: $CURRENT_VERSION"

# Parse version components
IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT_VERSION"

# Determine new version
BUMP_TYPE=${1:-patch}

case $BUMP_TYPE in
    patch)
        NEW_PATCH=$((PATCH + 1))
        NEW_VERSION="$MAJOR.$MINOR.$NEW_PATCH"
        ;;
    minor)
        NEW_MINOR=$((MINOR + 1))
        NEW_VERSION="$MAJOR.$NEW_MINOR.0"
        ;;
    major)
        NEW_MAJOR=$((MAJOR + 1))
        NEW_VERSION="$NEW_MAJOR.0.0"
        ;;
    *)
        echo "Usage: $0 [patch|minor|major]"
        echo "  patch: 1.0.0 -> 1.0.1"
        echo "  minor: 1.0.0 -> 1.1.0"
        echo "  major: 1.0.0 -> 2.0.0"
        exit 1
        ;;
esac

echo "New version: $NEW_VERSION"

# Update Cargo.toml
sed -i.bak "s/^version = \".*\"/version = \"$NEW_VERSION\"/" Cargo.toml
rm Cargo.toml.bak

echo "✅ Updated Cargo.toml to version $NEW_VERSION"

# Update brew formula if it exists
if [ -f "porter.rb" ]; then
    echo "📦 Updating brew formula..."
    ./scripts/update-brew-formula.sh
fi

# Update CHANGELOG.md if it exists
if [ -f "CHANGELOG.md" ]; then
    echo "📝 Please update CHANGELOG.md with the new version $NEW_VERSION"
fi

echo ""
echo "🎉 Version bumped to $NEW_VERSION"
echo ""
echo "Next steps:"
echo "1. Review changes: git diff"
echo "2. Commit changes: git add . && git commit -m \"Bump version to $NEW_VERSION\""
echo "3. Create tag: git tag v$NEW_VERSION"
echo "4. Push changes: git push && git push --tags"
echo "5. Create GitHub release: gh release create v$NEW_VERSION"
