// @ts-nocheck
import type { CollectionConfig } from 'payload'

import { authenticated } from '../../access/authenticated'
import { authenticatedOrPublished } from '../../access/authenticatedOrPublished'
import { slugField } from '@/fields/slug'
import { populatePublishedAt } from '../../hooks/populatePublishedAt'
import { generatePreviewPath } from '../../utilities/generatePreviewPath'
import { revalidateDelete, revalidateHotel } from './hooks/revalidateHotel'
import { tags } from '@/fields/tags'
import {
    MetaDescriptionField,
    MetaImageField,
    MetaTitleField,
    OverviewField,
    PreviewField,
} from '@payloadcms/plugin-seo/fields'

import { hero } from '@/heros/config'
import { block } from '@/fields/blocks'
import { address } from '@/fields/address'

export const Hotels: CollectionConfig<'hotels'> = {
    slug: 'hotels',
    access: {
        create: authenticated,
        delete: authenticated,
        read: authenticatedOrPublished,
        update: authenticated,
    },
    // This config controls what's populated by default when a page is referenced
    // https://payloadcms.com/docs/queries/select#defaultpopulate-collection-config-property
    // Type safe if the collection slug generic is passed to `CollectionConfig` - `CollectionConfig<'pages'>
    defaultPopulate: {
        title: true,
        slug: true,
    },
    admin: {
        defaultColumns: ['title', 'slug', 'updatedAt'],
        livePreview: {
            url: ({ data, req }) => {
                const path = generatePreviewPath({
                    slug: typeof data?.slug === 'string' ? data.slug : '',
                    collection: 'hotels',
                    req,
                })

                return path
            },
        },
        preview: (data, { req }) =>
            generatePreviewPath({
                slug: typeof data?.slug === 'string' ? data.slug : '',
                collection: 'hotels',
                req,
            }),
        useAsTitle: 'title',
    },
    fields: [
        {
            name: 'title',
            type: 'text',
            required: true,
        },
        {
            name: 'featureImage',
            type: 'upload',
            relationTo: 'media',
            required: true,
        },
        {
            name: 'countries',
            type: 'relationship',
            relationTo: 'countries',
            required: true,
            admin: {
                position: 'sidebar',
            },
        },
        {
            name: 'locals',
            type: 'relationship',
            relationTo: 'locals',
            admin: {
                position: 'sidebar',
            },
        },
        {
            name: 'coordinates',
            type: 'point',
            required: true,
            admin: {
                position: 'sidebar',
            },
        },
        tags({ name: 'feature_tags', label: 'Feature Tags', sidebar: true }),
        address({ sidebar: true }),
        {
            name: 'amenities',
            type: 'relationship',
            relationTo: 'amenities',
            hasMany: true,
        },
        {
            name: 'priceRange',
            type: 'select',
            options: [
                {
                    label: 'One',
                    value: 'one',
                },
                {
                    label: 'Two',
                    value: 'two',
                },
                {
                    label: 'Three',
                    value: 'three',
                },
            ],
        },
        {
            name: 'gallery',
            type: 'upload',
            relationTo: 'media',
            hasMany: true,
        },
        {
            name: 'excerpt',
            type: 'richText',
            required: true,
            admin: {
                description: 'This is the excerpt',
            },
        },
        {
            name: 'starRating',
            type: 'number',
            min: 1,
            max: 5,
            admin: {
                description: 'Hotel star rating (1-5)',
                position: 'sidebar',
            },
        },
        {
            name: 'budget',
            type: 'select',
            options: [
                {
                    label: 'Low',
                    value: 'low',
                },
                {
                    label: 'Medium',
                    value: 'medium',
                },
                {
                    label: 'High',
                    value: 'high',
                },
            ],
            admin: {
                position: 'sidebar',
            },
        },
        {
            name: 'brandTags',
            type: 'select',
            hasMany: true,
            options: [
                {
                    label: 'Boutique',
                    value: 'boutique',
                },
                {
                    label: 'Luxury',
                    value: 'luxury',
                },
                {
                    label: 'Eco-friendly',
                    value: 'eco-friendly',
                },
                {
                    label: 'Family-friendly',
                    value: 'family-friendly',
                },
                {
                    label: 'Adventure',
                    value: 'adventure',
                },
            ],
            admin: {
                position: 'sidebar',
            },
        },
        {
            name: 'holidayWhatTags',
            type: 'select',
            hasMany: true,
            options: [
                {
                    label: 'Tailormade Journeys',
                    value: 'tailormade-journeys',
                },
                {
                    label: 'Safari and Beach',
                    value: 'safari-and-beach',
                },
                {
                    label: 'Once In A Lifetime',
                    value: 'once-in-a-lifetime',
                },
                {
                    label: 'Wildlife and Nature',
                    value: 'wildlife-and-nature',
                },
            ],
            admin: {
                position: 'sidebar',
            },
        },
        {
            name: 'holidayWhoTags',
            type: 'select',
            hasMany: true,
            options: [
                {
                    label: 'Couples',
                    value: 'couples',
                },
                {
                    label: 'Honeymoons',
                    value: 'honeymoons',
                },
                {
                    label: 'Celebrations',
                    value: 'celebrations',
                },
                {
                    label: 'Families',
                    value: 'families',
                },
                {
                    label: 'Solo Travelers',
                    value: 'solo-travelers',
                },
            ],
            admin: {
                position: 'sidebar',
            },
        },
        {
            name: 'offers',
            type: 'text',
            admin: {
                description: 'Special offers or promotions',
                position: 'sidebar',
            },
        },
        {
            name: 'rcasHotelCode',
            type: 'text',
            admin: {
                description: 'Hotel booking code',
                position: 'sidebar',
            },
        },
        {
            name: 'featured',
            type: 'checkbox',
            defaultValue: false,
            admin: {
                position: 'sidebar',
            },
        },
        {
            name: 'sortOrder',
            type: 'number',
            defaultValue: 0,
            admin: {
                position: 'sidebar',
            },
        },
        {
            name: 'listingDescription',
            type: 'textarea',
            admin: {
                description: 'Short description for listings',
            },
        },
        {
            name: 'listingImage',
            type: 'upload',
            relationTo: 'media',
            admin: {
                description: 'Image for listings and thumbnails',
            },
        },
        {
            name: 'navigationTitle',
            type: 'text',
            admin: {
                description: 'Title used in navigation',
            },
        },
        {
            name: 'excludeFromBreadcrumb',
            type: 'checkbox',
            defaultValue: false,
            admin: {
                position: 'sidebar',
            },
        },
        {
            name: 'excludeFromSearchResults',
            type: 'checkbox',
            defaultValue: false,
            admin: {
                position: 'sidebar',
            },
        },
        {
            name: 'excludeFromSitemap',
            type: 'checkbox',
            defaultValue: false,
            admin: {
                position: 'sidebar',
            },
        },
        {
            name: 'isBlueprint',
            type: 'checkbox',
            defaultValue: false,
            admin: {
                position: 'sidebar',
            },
        },
        {
            name: 'bodyScripts',
            type: 'textarea',
            admin: {
                description: 'Custom scripts for body',
                position: 'sidebar',
            },
        },
        {
            name: 'headerScripts',
            type: 'textarea',
            admin: {
                description: 'Custom scripts for header',
                position: 'sidebar',
            },
        },
        {
            name: 'additionalMetaTags',
            type: 'textarea',
            admin: {
                description: 'Additional meta tags',
                position: 'sidebar',
            },
        },
        {
            name: 'openGraphTags',
            type: 'textarea',
            admin: {
                description: 'Open Graph tags',
                position: 'sidebar',
            },
        },
        {
            name: 'twitterCardTags',
            type: 'textarea',
            admin: {
                description: 'Twitter Card tags',
                position: 'sidebar',
            },
        },
        {
            name: 'htmlLanguageTag',
            type: 'text',
            admin: {
                description: 'HTML language tag',
                position: 'sidebar',
            },
        },
        {
            name: 'breadcrumb',
            type: 'text',
            admin: {
                description: 'Breadcrumb text',
                position: 'sidebar',
            },
        },
        {
            name: 'rcasAddressLine1',
            type: 'text',
            admin: {
                description: 'RCAS address line 1',
                position: 'sidebar',
            },
        },
        {
            name: 'rcasCity',
            type: 'text',
            admin: {
                description: 'RCAS city',
                position: 'sidebar',
            },
        },
        {
            name: 'rcasDescription',
            type: 'textarea',
            admin: {
                description: 'RCAS description',
                position: 'sidebar',
            },
        },
        {
            name: 'rcasName',
            type: 'text',
            admin: {
                description: 'RCAS name',
                position: 'sidebar',
            },
        },
        {
            name: 'rcasPostcode',
            type: 'text',
            admin: {
                description: 'RCAS postcode',
                position: 'sidebar',
            },
        },
        {
            name: 'seoKeywords',
            type: 'textarea',
            admin: {
                description: 'SEO keywords',
                position: 'sidebar',
            },
        },
        {
            name: 'umbracoUrlAlias',
            type: 'text',
            admin: {
                description: 'Umbraco URL alias',
                position: 'sidebar',
            },
        },
        {
            type: 'tabs',
            tabs: [
                {
                    fields: [hero],
                    label: 'Hero',
                },
                {
                    fields: [block()],
                    label: 'Content',
                },
                {
                    name: 'meta',
                    label: 'SEO',
                    fields: [
                        OverviewField({
                            titlePath: 'meta.title',
                            descriptionPath: 'meta.description',
                            imagePath: 'meta.image',
                        }),
                        MetaTitleField({
                            hasGenerateFn: true,
                        }),
                        MetaImageField({
                            relationTo: 'media',
                        }),

                        MetaDescriptionField({}),
                        PreviewField({
                            // if the `generateUrl` function is configured
                            hasGenerateFn: true,

                            // field paths to match the target field for data
                            titlePath: 'meta.title',
                            descriptionPath: 'meta.description',
                        }),
                    ],
                },
            ],
        },
        {
            name: 'publishedAt',
            type: 'date',
            admin: {
                position: 'sidebar',
            },
        },
        ...slugField(),
    ],
    hooks: {
        afterChange: [revalidateHotel],
        beforeChange: [populatePublishedAt],
        afterDelete: [revalidateDelete],
    },
    versions: {
        drafts: {
            autosave: {
                interval: 100, // We set this interval for optimal live preview
            },
            schedulePublish: true,
        },
        maxPerDoc: 50,
    },
}
