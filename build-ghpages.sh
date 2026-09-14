#!/bin/bash
set -e
{
  n=0
  decls=""
  while IFS= read -r f; do
    n=$((n+1))
    rel="${f#gh-pages-stage/}"
    printf 'blob\nmark :%s\ndata %s\n' "$n" "$(stat -c%s "$f")"
    cat "$f"
    printf '\n'
    decls+="M 100644 :$n $rel"$'\n'
  done < <(find gh-pages-stage -type f | sort)
  printf 'commit refs/heads/gh-pages\n'
  printf 'committer deploy <deploy@localhost> %s +0000\n' "$(date +%s)"
  msg='web build'
  printf 'data %s\n%s\n' "${#msg}" "$msg"
  printf 'deleteall\n'
  printf '%s' "$decls"
  printf 'done\n'
} | git fast-import --quiet
git log --oneline -1 gh-pages
git ls-tree -r --name-only gh-pages | wc -l
