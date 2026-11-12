<?php
/**
 * Porter's generic WordPress exporter, run by the wp-db connector with `wp eval-file`.
 * Everything is discovered at runtime: no site, post type, address or plugin-specific content
 * is hardcoded. Output is one JSON object written to $PORTER_OUT (never stdout, which PHP
 * notices can pollute), via a temp file renamed at the end so a crash leaves nothing half-written.
 * Form submissions and Redirection rows are read by the connector with SQL, not here.
 */
error_reporting(E_ERROR | E_PARSE | E_CORE_ERROR | E_COMPILE_ERROR);
require_once ABSPATH . 'wp-admin/includes/post.php';
global $wpdb;

$target = getenv('PORTER_OUT') ?: '/out/export.json';
$part = $target . '.part';
$fh = fopen($part, 'w');
if (!$fh) {
	fwrite(STDERR, "porter: cannot write $part\n");
	exit(1);
}
$json = function ($v) {
	$s = json_encode($v, JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE | JSON_INVALID_UTF8_SUBSTITUTE | JSON_PARTIAL_OUTPUT_ON_ERROR);
	return $s === false ? 'null' : $s;
};
$opened = false;
$key = function ($name) use ($fh, $json, &$opened) {
	fwrite($fh, ($opened ? ',' : '{') . $json($name) . ':');
	$opened = true;
};
$section = function ($name, $v) use ($fh, $json, $key) {
	$key($name);
	fwrite($fh, $json($v));
};

// ACF: unformatted values (media and relations stay ids); nested rows come back keyed by
// field_xxx, so every such key is renamed to its field name.
$has_acf = function_exists('get_fields') && function_exists('acf_get_field');
$key_names = [];
$names = function ($v) use (&$names, &$key_names) {
	if (!is_array($v)) return $v;
	$o = [];
	foreach ($v as $k => $x) {
		if (is_string($k) && strpos($k, 'field_') === 0) {
			if (!array_key_exists($k, $key_names)) {
				$f = acf_get_field($k);
				$key_names[$k] = is_array($f) && !empty($f['name']) ? $f['name'] : $k;
			}
			$k = $key_names[$k];
		}
		$o[$k] = $names($x);
	}
	return $o;
};
$acf = function ($id) use ($has_acf, $names) {
	if (!$has_acf) return null;
	$v = get_fields($id, false);
	return (object) $names(is_array($v) ? $v : []);
};

$meta_of = function ($all) {
	$o = [];
	foreach ($all ?: [] as $k => $vals) {
		$vals = array_map('maybe_unserialize', (array) $vals);
		$o[$k] = count($vals) === 1 ? $vals[0] : $vals;
	}
	return (object) $o;
};

// Non-public posts (private, draft, future) get a plain ?p= link from get_permalink; asking
// for the link of a published copy (the way the editor's sample permalink does) gives the
// pretty path they would have.
$link = function (WP_Post $p) {
	if ($p->post_type === 'attachment') return wp_get_attachment_url($p->ID) ?: null;
	$l = get_permalink($p);
	if (!$l || strpos($l, '?') === false) return $l ?: null;
	$q = clone $p;
	$q->post_status = 'publish';
	$q->filter = 'sample';
	if (!$q->post_name) {
		$q->post_name = wp_unique_post_slug(sanitize_title($q->post_title ?: (string) $q->ID), $q->ID, 'publish', $q->post_type, $q->post_parent);
	}
	return get_permalink($q) ?: $l;
};

$site = [
	'home' => home_url(),
	'siteurl' => site_url(),
	'blogname' => get_option('blogname'),
	'blogdescription' => get_option('blogdescription'),
	'show_on_front' => get_option('show_on_front'),
	'page_on_front' => (int) get_option('page_on_front') ?: null,
	'page_for_posts' => (int) get_option('page_for_posts') ?: null,
	'permalink_structure' => get_option('permalink_structure'),
	'wpseo_titles' => get_option('wpseo_titles') ?: null,
	'active_plugins' => array_values((array) get_option('active_plugins', [])),
	'acf_active' => $has_acf,
	'acf_options' => $acf('option'),
];
$section('site', $site);

// Post types: registered ones plus any still in the table whose plugin is gone. Revisions and
// menu items are skipped (menus are exported below); Ninja Forms submissions are read as
// submissions by the connector.
$skip = ['revision', 'nav_menu_item', 'nf_sub'];
$registered = get_post_types([], 'objects');
$type_names = array_unique(array_merge(array_keys($registered), $wpdb->get_col("SELECT DISTINCT post_type FROM {$wpdb->posts}")));
$type_names = array_values(array_diff($type_names, $skip));
$types = [];
foreach ($type_names as $t) {
	$o = $registered[$t] ?? null;
	$types[$t] = $o ? [
		'label' => $o->label,
		'public' => (bool) $o->public,
		'hierarchical' => (bool) $o->hierarchical,
		'rewrite' => $o->rewrite,
		'taxonomies' => get_object_taxonomies($t),
	] : null;
}
$section('post_types', (object) $types);

// Posts, streamed one at a time.
$in = implode(',', array_map(fn($t) => "'" . esc_sql($t) . "'", $type_names ?: ['']));
$ids = $wpdb->get_col("SELECT ID FROM {$wpdb->posts} WHERE post_status <> 'auto-draft' AND post_type IN ($in) ORDER BY ID");
$pll = function_exists('pll_get_post_translations');
$pll_groups = [];
$key('posts');
fwrite($fh, '[');
$written = 0;
foreach ($ids as $id) {
	$p = get_post((int) $id);
	if (!$p) continue;
	$terms = [];
	foreach (get_object_taxonomies($p->post_type) as $tax) {
		$t = wp_get_object_terms($p->ID, $tax, ['fields' => 'ids']);
		if (!is_wp_error($t) && $t) $terms[$tax] = array_map('intval', $t);
	}
	$item = [
		'id' => (int) $p->ID,
		'type' => $p->post_type,
		'status' => $p->post_status,
		'slug' => $p->post_name,
		'title' => $p->post_title,
		'link' => $link($p),
		'parent' => (int) $p->post_parent ?: null,
		'menu_order' => (int) $p->menu_order,
		'author' => (int) $p->post_author ?: null,
		'date' => $p->post_date,
		'date_gmt' => $p->post_date_gmt,
		'modified_gmt' => $p->post_modified_gmt,
		'mime' => $p->post_mime_type ?: null,
		'template' => get_page_template_slug($p) ?: null,
		'featured_image' => (int) get_post_thumbnail_id($p) ?: null,
		'content' => $p->post_content,
		'excerpt' => $p->post_excerpt,
		'meta' => $meta_of(get_post_meta($p->ID)),
		'acf' => $acf($p->ID),
		'terms' => (object) $terms,
	];
	if ($p->post_type === 'attachment') {
		$item['attachment'] = [
			'file' => get_post_meta($p->ID, '_wp_attached_file', true) ?: null,
			'metadata' => wp_get_attachment_metadata($p->ID) ?: null,
		];
	}
	if ($pll) {
		$item['lang'] = pll_get_post_language($p->ID) ?: null;
		$tr = pll_get_post_translations($p->ID);
		if (is_array($tr) && count($tr) > 1) {
			asort($tr);
			$pll_groups[implode('-', $tr)] = $tr;
		}
	}
	fwrite($fh, ($written ? ',' : '') . $json($item));
	if (++$written % 200 === 0) wp_cache_flush();
}
fwrite($fh, ']');

// Taxonomies and terms (menus are their own section).
$taxes = [];
foreach (get_taxonomies([], 'objects') as $name => $t) {
	$taxes[$name] = [
		'label' => $t->label,
		'public' => (bool) $t->public,
		'hierarchical' => (bool) $t->hierarchical,
		'object_types' => array_values((array) $t->object_type),
		'rewrite' => $t->rewrite,
	];
}
$section('taxonomies', (object) $taxes);
$terms = [];
$tax_names = array_values(array_diff(array_keys($taxes), ['nav_menu']));
$found = $tax_names ? get_terms(['taxonomy' => $tax_names, 'hide_empty' => false]) : [];
foreach (is_wp_error($found) ? [] : $found as $t) {
	$l = get_term_link($t);
	$terms[] = [
		'id' => (int) $t->term_id,
		'taxonomy' => $t->taxonomy,
		'slug' => $t->slug,
		'name' => $t->name,
		'description' => $t->description,
		'parent' => (int) $t->parent ?: null,
		'count' => (int) $t->count,
		'link' => is_wp_error($l) ? null : $l,
		'meta' => $meta_of(get_term_meta($t->term_id)),
		'acf' => $acf("term_{$t->term_id}"),
	];
}
$section('terms', $terms);

$users = [];
foreach (get_users(['orderby' => 'ID']) as $u) {
	$users[] = [
		'id' => (int) $u->ID,
		'login' => $u->user_login,
		'nicename' => $u->user_nicename,
		'display_name' => $u->display_name,
		'email' => $u->user_email,
		'roles' => array_values((array) $u->roles),
	];
}
$section('users', $users);

// Menus with their theme locations.
$locations = get_nav_menu_locations();
$menus = [];
foreach (wp_get_nav_menus() as $m) {
	$items = [];
	foreach (wp_get_nav_menu_items($m) ?: [] as $i) {
		$items[] = [
			'id' => (int) $i->ID,
			'parent' => (int) $i->menu_item_parent ?: null,
			'order' => (int) $i->menu_order,
			'title' => $i->title,
			'url' => $i->url,
			'type' => $i->type,
			'object' => $i->object,
			'object_id' => (int) $i->object_id ?: null,
			'target' => $i->target,
			'classes' => array_values(array_filter((array) $i->classes)),
		];
	}
	$menus[] = [
		'id' => (int) $m->term_id,
		'name' => $m->name,
		'slug' => $m->slug,
		'locations' => array_keys(array_filter($locations, fn($id) => (int) $id === (int) $m->term_id)),
		'items' => $items,
	];
}
$section('menus', $menus);
$section('menu_locations', (object) get_registered_nav_menus());

$groups = [];
if (function_exists('acf_get_field_groups') && function_exists('acf_get_fields')) {
	foreach (acf_get_field_groups() as $g) {
		$g['fields'] = acf_get_fields($g) ?: [];
		$groups[] = $g;
	}
}
$section('acf_field_groups', $groups);

$forms = ['gf' => [], 'nf' => []];
if (class_exists('GFAPI')) {
	$forms['gf'] = array_values(GFAPI::get_forms(null) ?: []);
}
if (function_exists('Ninja_Forms')) {
	foreach (Ninja_Forms()->form()->get_forms() as $f) {
		$id = $f->get_id();
		$forms['nf'][] = [
			'id' => (int) $id,
			'settings' => $f->get_settings(),
			'fields' => array_values(array_map(fn($x) => ['id' => $x->get_id()] + $x->get_settings(), Ninja_Forms()->form($id)->get_fields())),
			'actions' => array_values(array_map(fn($x) => ['id' => $x->get_id()] + $x->get_settings(), Ninja_Forms()->form($id)->get_actions())),
		];
	}
}
$section('forms', $forms);

$translations = null;
if (defined('ICL_SITEPRESS_VERSION')) {
	$translations = [
		'plugin' => 'wpml',
		'rows' => $wpdb->get_results("SELECT element_id, element_type, trid, language_code, source_language_code FROM {$wpdb->prefix}icl_translations", ARRAY_A),
	];
} elseif ($pll) {
	$translations = ['plugin' => 'polylang', 'groups' => array_values($pll_groups)];
}
$section('translations', $translations);

fwrite($fh, '}');
if (!fclose($fh) || !rename($part, $target)) {
	fwrite(STDERR, "porter: could not finish $target\n");
	exit(1);
}
fwrite(STDERR, "porter: wrote $written posts\n");
