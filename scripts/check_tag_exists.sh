#!/bin/bash
dist_file_name="$1"
echo "dist_file_name=$dist_file_name"

version=$(echo "$dist_file_name" | sed -E 's/^goose-ai-([0-9]+\.[0-9]+\.[0-9]+)\.tar\.gz$/\1/')
echo "version=$version"

if [ "$version" != "$dist_file_name" ]; then
    git fetch --tags
    tag_exists=$(git tag --list "v$version")
    echo "tag_exists=$tag_exists"
fi
if [ -z "$tag_exists" ]; then
    echo "version $version is not tagged yet"
    echo "tag_version=$version" >> "$GITHUB_ENV"
else
    echo "Tag exists"
fi
