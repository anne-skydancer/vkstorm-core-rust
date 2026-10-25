#!/bin/sh
# Installs the repository's git hooks into the directory git uses for hooks
# (the shared .git/hooks, or core.hooksPath when it is set), so they apply to
# the main checkout and to every worktree.
#   pre-merge-commit: refuse to merge a base line into a feature branch
#   pre-commit:       the same check when a conflicted merge is concluded by
#                     "git commit"
# A hook of either name from elsewhere is kept as <name>.local and called after
# the check. When core.hooksPath is the repository's own scripts/git-hooks, the
# tracked hooks are used in place and nothing is copied.
set -e
root=$(git rev-parse --show-toplevel)
# A relative core.hooksPath is relative to the worktree root.
cd "$root"
hooks=$(git rev-parse --git-path hooks)
case "$hooks" in
    /*|[A-Za-z]:*) ;;
    *) hooks="$root/$hooks" ;;
esac
mkdir -p "$hooks"
if [ "$hooks/pre-merge-commit" -ef "$root/scripts/git-hooks/pre-merge-commit" ]; then
    chmod +x "$root/scripts/git-hooks/pre-merge-commit" "$root/scripts/git-hooks/pre-commit"
    echo "core.hooksPath is scripts/git-hooks: the tracked hooks are used in place"
    exit 0
fi
for name in pre-merge-commit pre-commit; do
    # Ours (any version) is replaced; anything else is kept and chained.
    if [ -f "$hooks/$name" ] && ! grep -q -e "vks-base-merge-check" -e "check_no_base_merges" "$hooks/$name"; then
        mv "$hooks/$name" "$hooks/$name.local"
    fi
    cp "$root/scripts/git-hooks/$name" "$hooks/$name"
    chmod +x "$hooks/$name"
done
cp "$root/scripts/check_no_base_merges.py" "$hooks/check_no_base_merges.py"
echo "Installed pre-merge-commit and pre-commit into $hooks"
