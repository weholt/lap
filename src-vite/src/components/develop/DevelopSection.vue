<script setup lang="ts">
import { useI18n } from 'vue-i18n';

import { IconRestore } from '@/common/icons';

/**
 * Collapsible develop section (lap-adc): bypass checkbox bound to the
 * recipe's sectionVisibility (engine enable/bypass state), localized title,
 * per-section reset, and a slot for the section's controls.
 */
defineProps<{
    sectionId: string;
    labelKey: string;
    visible: boolean;
    expanded: boolean;
}>();

const emit = defineEmits<{
    'toggle-expand': [];
    'toggle-visible': [value: boolean];
    reset: [];
}>();

const { t } = useI18n();
</script>

<template>
    <div
        class="border-t border-base-content/5"
        :data-testid="`develop-section-${sectionId}`"
        :class="{ 'opacity-50': !visible }"
    >
        <div class="flex items-center gap-1 px-1 pt-2 pb-1">
            <input
                type="checkbox"
                class="checkbox checkbox-xs checkbox-primary"
                :checked="visible"
                :data-testid="`develop-bypass-${sectionId}`"
                :aria-label="$t('develop.bypassSection', { section: $t(labelKey) })"
                :title="$t('develop.bypassSection', { section: $t(labelKey) })"
                @change="emit('toggle-visible', ($event.target as HTMLInputElement).checked)"
            />
            <button
                type="button"
                class="flex-1 flex items-center gap-1 text-left min-w-0"
                :data-testid="`develop-section-toggle-${sectionId}`"
                :aria-expanded="expanded ? 'true' : 'false'"
                :aria-label="$t('develop.sectionAria', { section: $t(labelKey) })"
                @click.stop="emit('toggle-expand')"
            >
                <svg class="w-3.5 h-3.5 shrink-0 text-base-content/50" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true">
                    <template v-if="sectionId === 'basic'"><circle cx="12" cy="12" r="4"/><path d="M12 1v3m0 16v3M1 12h3m16 0h3M4 4l2 2m12 12 2 2M20 4l-2 2M6 18l-2 2"/></template>
                    <template v-else-if="sectionId === 'curves'"><path d="M3 20h18M3 20V4M4 18c5 0 4-9 9-9s3-5 8-5"/></template>
                    <template v-else-if="sectionId === 'color'"><circle cx="12" cy="12" r="9"/><path d="M12 3v9l8 4M12 12l-8 5"/></template>
                    <template v-else-if="sectionId === 'details'"><path d="m12 2 2.5 7.5L22 12l-7.5 2.5L12 22l-2.5-7.5L2 12l7.5-2.5Z"/></template>
                    <template v-else-if="sectionId === 'effects'"><path d="m12 2 2 6 6 2-6 2-2 6-2-6-6-2 6-2ZM19 17l.8 2.2L22 20l-2.2.8L19 23l-.8-2.2L16 20l2.2-.8Z"/></template>
                    <template v-else-if="sectionId === 'vignetting'"><circle cx="12" cy="12" r="9"/><circle cx="12" cy="12" r="5"/><circle cx="12" cy="12" r="1"/></template>
                    <template v-else><path d="M4 4h16v16H4zM8 8h8v8H8z"/></template>
                </svg>
                <span
                    class="font-bold uppercase text-[11px] tracking-wide text-base-content/40 truncate"
                    :data-testid="`develop-section-title-${sectionId}`"
                >{{ $t(labelKey) }}</span>
            </button>
            <button
                type="button"
                class="btn btn-ghost btn-xs text-base-content/50 hover:text-base-content"
                :data-testid="`develop-section-reset-${sectionId}`"
                :title="$t('develop.resetSection', { section: $t(labelKey) })"
                :aria-label="$t('develop.resetSection', { section: $t(labelKey) })"
                @click.stop="emit('reset')"
            >
                <IconRestore class="w-3 h-3" />
            </button>
        </div>
        <div v-show="expanded" class="pb-1">
            <slot />
        </div>
    </div>
</template>
