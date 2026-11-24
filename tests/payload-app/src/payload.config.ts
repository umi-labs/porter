// Minimal Payload app for porter's golden tests. Not a real site: no frontend.
import { sqliteAdapter } from '@payloadcms/db-sqlite'
import { formBuilderPlugin } from '@payloadcms/plugin-form-builder'
import { nestedDocsPlugin } from '@payloadcms/plugin-nested-docs'
import { redirectsPlugin } from '@payloadcms/plugin-redirects'
import { lexicalEditor } from '@payloadcms/richtext-lexical'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { buildConfig } from 'payload'
// Generated from the Sanity fixture by `porter schema generate --into` (golden-tested).
import { porterCollections } from './collections/porter'

const dirname = path.dirname(fileURLToPath(import.meta.url))
const root = path.resolve(dirname, '..')

const legacyId = { name: 'legacyId', type: 'text', unique: true } as const
const slug = { name: 'slug', type: 'text', index: true } as const

export default buildConfig({
  secret: process.env.PAYLOAD_SECRET || 'porter-test-secret-not-for-production',
  db: sqliteAdapter({
    client: { url: process.env.DATABASE_URI || `file:${path.join(root, 'porter-test.db')}` },
  }),
  editor: lexicalEditor(),
  admin: { user: 'users', importMap: { baseDir: dirname } },
  typescript: { autoGenerate: false, outputFile: path.join(dirname, 'payload-types.ts') },
  collections: [
    {
      slug: 'users',
      auth: { useAPIKey: true },
      fields: [],
    },
    {
      slug: 'media',
      folders: true,
      upload: { staticDir: path.join(root, 'media') },
      fields: [{ name: 'alt', type: 'text' }, legacyId],
    },
    {
      slug: 'pages',
      versions: { drafts: true },
      fields: [
        { name: 'title', type: 'text', required: true },
        slug,
        legacyId,
        {
          name: 'layout',
          type: 'blocks',
          blocks: [
            { slug: 'richText', fields: [{ name: 'content', type: 'richText' }] },
            {
              slug: 'image',
              fields: [
                { name: 'image', type: 'upload', relationTo: 'media' },
                { name: 'caption', type: 'text' },
              ],
            },
            { slug: 'raw', fields: [{ name: 'html', type: 'textarea' }] },
          ],
        },
        {
          name: 'seo',
          type: 'group',
          fields: [
            { name: 'title', type: 'text' },
            { name: 'description', type: 'textarea' },
            { name: 'image', type: 'upload', relationTo: 'media' },
          ],
        },
      ],
    },
    {
      slug: 'posts',
      versions: { drafts: true },
      fields: [
        { name: 'title', type: 'text', required: true },
        slug,
        legacyId,
        { name: 'content', type: 'richText' },
      ],
    },
    ...porterCollections,
  ],
  globals: [
    {
      slug: 'navigation',
      fields: [
        {
          name: 'main',
          type: 'array',
          fields: [
            { name: 'label', type: 'text' },
            { name: 'url', type: 'text' },
            {
              name: 'children',
              type: 'array',
              fields: [
                { name: 'label', type: 'text' },
                { name: 'url', type: 'text' },
              ],
            },
          ],
        },
      ],
    },
  ],
  plugins: [
    nestedDocsPlugin({ collections: ['pages'] }),
    formBuilderPlugin({}),
    redirectsPlugin({ collections: ['pages', 'posts'] }),
  ],
})
