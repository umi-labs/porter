#!/bin/bash

# Script to update the brew formula with the current version
# This script should be run from the porter repository root

set -e

# Get the current version from Cargo.toml
VERSION=$(grep '^version = ' Cargo.toml | sed 's/version = "\(.*\)"/\1/')
echo "Current version: $VERSION"

# Create a release tarball
TARBALL="porter-${VERSION}.tar.gz"
echo "Creating tarball: $TARBALL"

# Create the tarball (excluding git files and build artifacts)
tar --exclude='.git' \
    --exclude='target' \
    --exclude='*.tar.gz' \
    --exclude='.DS_Store' \
    -czf "$TARBALL" .

# Calculate SHA256
SHA256=$(shasum -a 256 "$TARBALL" | cut -d' ' -f1)
echo "SHA256: $SHA256"

# Update the formula file
FORMULA_FILE="porter.rb"
if [ -f "$FORMULA_FILE" ]; then
    # Update version
    sed -i.bak "s/url \"https:\/\/github\.com\/umi-labs\/porter\/archive\/v.*\.tar\.gz\"/url \"https:\/\/github.com\/umi-labs\/porter\/archive\/v${VERSION}.tar.gz\"/" "$FORMULA_FILE"
    
    # Update SHA256
    sed -i.bak "s/sha256 \".*\"/sha256 \"${SHA256}\"/" "$FORMULA_FILE"
    
    # Remove backup file
    rm "${FORMULA_FILE}.bak"
    
    echo "Updated $FORMULA_FILE with version $VERSION and SHA256 $SHA256"
else
    echo "Formula file $FORMULA_FILE not found"
    exit 1
fi

echo "✅ Brew formula updated successfully!"
echo "📦 Tarball created: $TARBALL"
echo "🔧 Formula updated: $FORMULA_FILE"
echo ""
echo "Next steps:"
echo "1. Create a git tag: git tag v$VERSION"
echo "2. Push the tag: git push origin v$VERSION"
echo "3. Upload the tarball to GitHub releases"
echo "4. Update your homebrew tap with the new formula"
