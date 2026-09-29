<script setup lang="ts">
import { computed, ref, useId } from 'vue';
import { useI18n } from 'vue-i18n';
import { IconRestore } from '@/common/icons';
import { useDevelopEditor } from '@/composables/useDevelopEditor';
import { DEFAULT_RECIPE, type LevelsChannel } from '@/composables/useDevelopSession.types';
import LevelsControls from './LevelsControls.vue';
import type { LevelsColor, LevelsPixels } from './levels';

defineProps<{ pixels?: LevelsPixels | null }>();
const { t } = useI18n();
const editor = useDevelopEditor();
const id = useId();
const expanded = ref(false);
const channel = ref<LevelsColor>('rgb');
const channels: LevelsColor[] = ['rgb', 'red', 'green', 'blue'];
const levels = computed(() => editor.recipe.value?.levels ?? DEFAULT_RECIPE.levels);
function change(value: LevelsChannel, live: boolean) {
    const patch = { levels: { ...levels.value, [channel.value]: value } };
    if (live) editor.applyRecipePatchLive(patch, `levels ${channel.value}`);
    else editor.applyRecipePatch(patch, `levels ${channel.value}`);
}
function select(value: LevelsColor) { editor.endEditTransaction(); channel.value = value; }
function toggle() { editor.endEditTransaction(); expanded.value = !expanded.value; }
function reset(all: boolean) {
    editor.endEditTransaction();
    editor.applyRecipePatch({ levels: all ? structuredClone(DEFAULT_RECIPE.levels) : { ...levels.value, [channel.value]: { ...DEFAULT_RECIPE.levels[channel.value] } } }, all ? 'reset levels' : `reset levels ${channel.value}`);
}
function enable(e: Event) {
    editor.applyRecipePatch({ levels: { ...levels.value, enabled: (e.target as HTMLInputElement).checked } }, 'bypass levels');
}
function tabKey(e: KeyboardEvent, index: number) {
    const next = e.key === 'ArrowRight' ? (index + 1) % 4 : e.key === 'ArrowLeft' ? (index + 3) % 4 : e.key === 'Home' ? 0 : e.key === 'End' ? 3 : null;
    if (next === null) return;
    e.preventDefault(); select(channels[next]);
    (e.currentTarget as HTMLElement).parentElement?.querySelectorAll<HTMLButtonElement>('[role="tab"]')[next]?.focus();
}
</script>

<template>
    <section class="border-t border-base-content/5" data-testid="develop-section-levels">
        <div class="flex items-center gap-1 px-1 pt-2 pb-1">
            <input type="checkbox" class="checkbox checkbox-primary checkbox-xs" :checked="levels.enabled" :disabled="!editor.session.value"
                :aria-label="t('develop.levels.enabled')" data-testid="levels-enabled" @change="enable" />
            <button type="button" class="flex-1 text-left text-[11px] font-bold uppercase text-base-content/60"
                :aria-expanded="expanded" :aria-controls="`${id}-body`" data-testid="develop-section-toggle-levels" @click="toggle">{{ t('develop.levels.title') }}</button>
            <button type="button" class="btn btn-ghost btn-xs text-base-content/50" :disabled="!editor.session.value"
                :title="t('develop.levels.resetAll')" :aria-label="t('develop.levels.resetAll')" data-testid="levels-reset-all" @click="reset(true)"><IconRestore class="w-3 h-3" /></button>
        </div>
        <div v-if="expanded" :id="`${id}-body`" class="px-1 pb-3">
            <div class="levels-tabs" role="tablist" :aria-label="t('develop.levels.title')">
                <button v-for="(item, index) in channels" :key="item" type="button" role="tab" :id="`${id}-${item}`"
                    :aria-controls="`${id}-channel`" :aria-selected="channel === item" :tabindex="channel === item ? 0 : -1"
                    :data-testid="`levels-tab-${item}`" @click="select(item)" @keydown.stop="tabKey($event, index)">{{ t(`develop.levels.${item}`) }}</button>
            </div>
            <div :id="`${id}-channel`" role="tabpanel" :aria-labelledby="`${id}-${channel}`">
                <LevelsControls :key="channel" :value="levels[channel]" :channel="channel" :pixels="pixels" :disabled="!editor.session.value"
                    @live="change($event, true)" @commit="change($event, false)" @settle="editor.endEditTransaction()" />
            </div>
            <div class="flex items-center justify-between mt-1">
                <span v-if="!levels.enabled" class="text-xs text-warning">{{ t('develop.levels.bypassed') }}</span><span v-else />
                <button type="button" class="btn btn-ghost btn-xs text-base-content/60" :disabled="!editor.session.value"
                    data-testid="levels-reset-channel" @click="reset(false)">{{ t('develop.levels.resetChannel') }}</button>
            </div>
        </div>
    </section>
</template>

<style scoped>
.levels-tabs { display: flex; border-bottom: 1px solid #8884; margin-bottom: 12px; }
.levels-tabs button { flex: 1; padding: 8px 0; font-size: 11px; opacity: .65; border-bottom: 2px solid transparent; }
.levels-tabs button[aria-selected='true'] { color: var(--color-primary, #ff7bc8); opacity: 1; border-bottom-color: currentColor; }
.levels-tabs button:focus-visible { outline: 1px solid var(--color-primary, #ff7bc8); }
</style>
