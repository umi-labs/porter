#!/usr/bin/env bash
# Rebuilds dump.sql.gz: a fresh, synthetic WordPress install (example.com) with 3 posts (one
# private), a page with a child page, an ACF field group with a value, a menu, 2 Redirection
# rows and a Yoast meta row. Needs Docker and network access. WordPress core and plugins are
# not committed; tests/wp_db.rs downloads them into target/ at test time (see `versions`).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=/dev/null
. "$here/versions"
tag="porter-fixture-$$"
net="$tag-net" db="$tag-db" vol="$tag-wp"
cleanup() {
  docker rm -f "$db" >/dev/null 2>&1 || true
  docker volume rm -f "$vol" >/dev/null 2>&1 || true
  docker network rm "$net" >/dev/null 2>&1 || true
}
trap cleanup EXIT

docker network create "$net" >/dev/null
docker run -d --name "$db" --network "$net" \
  -e MARIADB_ROOT_PASSWORD=fixture -e MARIADB_DATABASE=wp mariadb:11 >/dev/null
until docker exec "$db" mariadb -h127.0.0.1 -uroot -pfixture -e 'select 1' >/dev/null 2>&1; do sleep 1; done

wp() { docker run --rm -i --network "$net" -v "$vol:/var/www/html" -e HOME=/tmp wordpress:cli php -d memory_limit=1G /usr/local/bin/wp "$@"; }
sql() { docker exec -i "$db" mariadb -uroot -pfixture wp "$@"; }

# The named volume starts root-owned; hand it to www-data.
docker run --rm -u 0 -v "$vol:/var/www/html" wordpress:cli chown www-data:www-data /var/www/html
wp core download --version="$WP_VERSION" --quiet
wp config create --dbname=wp --dbuser=root --dbpass=fixture --dbhost="$db" --skip-check --quiet
wp core install --url=https://example.com --title="Porter Fixture" --admin_user=admin \
  --admin_password=fixture --admin_email=admin@example.com --skip-email --quiet
wp post delete $(wp post list --post_type=post,page --post_status=any --format=ids) --force --quiet
wp rewrite structure '/%postname%/' --quiet

cat_id=$(wp term create category News --porcelain)
first=$(wp post create --post_type=post --post_status=publish --post_title="First Post" \
  --post_content="<p>Hello from the first post.</p>" --post_category="$cat_id" --porcelain)
wp post create --post_type=post --post_status=publish --post_title="Second Post" \
  --post_content="<p>Second.</p>" --porcelain >/dev/null
wp post create --post_type=post --post_status=private --post_title="Private Post" \
  --post_name=private-post --post_content="<p>Members only.</p>" --porcelain >/dev/null
parent=$(wp post create --post_type=page --post_status=publish --post_title="Parent Page" --porcelain)
wp post create --post_type=page --post_status=publish --post_title="Child Page" \
  --post_parent="$parent" --porcelain >/dev/null

# Yoast stores per-post SEO in postmeta; the plugin itself is not needed for the row.
wp post meta add "$first" _yoast_wpseo_title "First Post SEO title" --quiet

wp plugin install advanced-custom-fields --version="$ACF_VERSION" --activate --quiet
wp eval "
acf_import_field_group([
  'key' => 'group_porter_details', 'title' => 'Post Details',
  'fields' => [['key' => 'field_porter_subtitle', 'label' => 'Subtitle', 'name' => 'subtitle', 'type' => 'text']],
  'location' => [[['param' => 'post_type', 'operator' => '==', 'value' => 'post']]],
]);
update_field('field_porter_subtitle', 'A subtitle', $first);
"

menu=$(wp menu create "Main" --porcelain)
wp menu item add-post "$menu" "$parent" --quiet
wp menu item add-custom "$menu" "Elsewhere" "https://example.com/elsewhere/" --quiet

wp plugin install redirection --version="$REDIRECTION_VERSION" --activate --quiet
# Redirection creates its tables from its admin screen or its own CLI command.
wp redirection database install --quiet
sql -e "INSERT INTO wp_redirection_items (url, match_url, match_data, regex, position, last_count, last_access, group_id, status, action_type, action_code, action_data, match_type, title)
VALUES ('/old-page/', '/old-page', NULL, 0, 0, 0, '1970-01-01 00:00:00', 1, 'enabled', 'url', 301, '/parent-page/', 'url', ''),
       ('/older/', '/older', NULL, 0, 1, 0, '1970-01-01 00:00:00', 1, 'enabled', 'url', 301, '/old-page/', 'url', '');"

docker exec "$db" mariadb-dump -uroot -pfixture --skip-dump-date wp | gzip -n > "$here/dump.sql.gz"
echo "wrote $here/dump.sql.gz ($(wc -c < "$here/dump.sql.gz") bytes)"
