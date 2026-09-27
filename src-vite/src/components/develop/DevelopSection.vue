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
