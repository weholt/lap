<script setup lang="ts">
import { computed, ref, useId } from 'vue';
import { useI18n } from 'vue-i18n';
import { IconRestore } from '@/common/icons';
import { useDevelopEditor } from '@/composables/useDevelopEditor';
import { DEFAULT_RECIPE, type HueSatLum } from '@/composables/useDevelopSession.types';
import ColorBalanceWheel from './ColorBalanceWheel.vue';
import DevelopSliderControl from './DevelopSliderControl.vue';

const { t } = useI18n();
const editor = useDevelopEditor();
const id = useId();
const expanded = ref(false);
const selected = ref('global');
const tabs = ['global', 'threeWay', 'shadows', 'midtones', 'highlights'] as const;
type Zone = 'global' | 'shadows' | 'midtones' | 'highlights';
const zones = computed<Zone[]>(() => selected.value === 'threeWay' ? ['shadows', 'midtones', 'highlights'] : [selected.value as Zone]);
const grading = computed(() => editor.recipe.value?.colorGrading ?? DEFAULT_RECIPE.colorGrading);
function label(tab: string) { return t(`develop.colorBalance.${tab}`); }
function change(zone: Zone, value: HueSatLum, live: boolean) {
    const patch = { colorGrading: { ...grading.value, [zone]: value } };
    if (live) editor.applyRecipePatchLive(patch, `color balance ${zone}`);
    else editor.applyRecipePatch(patch, `color balance ${zone}`);
}
function reset(zone?: Zone) {
    editor.applyRecipePatch({ colorGrading: zone ? { ...grading.value, [zone]: { ...DEFAULT_RECIPE.colorGrading[zone] } } : structuredClone(DEFAULT_RECIPE.colorGrading) }, zone ? `reset color balance ${zone}` : 'reset color balance');
}
function select(tab: string) {
    editor.endEditTransaction();
    selected.value = tab;
}
function tabKey(e: KeyboardEvent, index: number) {
    let next = index;
    if (e.key === 'ArrowRight') next = (index + 1) % tabs.length;
    else if (e.key === 'ArrowLeft') next = (index + tabs.length - 1) % tabs.length;
    else if (e.key === 'Home') next = 0;
    else if (e.key === 'End') next = tabs.length - 1;
    else return;
    e.preventDefault();
    select(tabs[next]);
    (e.currentTarget as HTMLElement).parentElement?.querySelectorAll<HTMLButtonElement>('[role="tab"]')[next]?.focus();
}
</script>

<template>
    <section class="color-balance border-t border-base-content/5" data-testid="develop-section-color-balance">
        <div class="flex items-center gap-1 px-1 pt-2 pb-1">
            <button class="flex-1 text-left text-[11px] font-bold uppercase text-base-content/60" type="button"
                data-testid="develop-section-toggle-color-balance" :aria-expanded="expanded" :aria-controls="`${id}-body`"
                @click="expanded = !expanded">{{ t('develop.colorBalance.title') }}</button>
            <button type="button" class="btn btn-ghost btn-xs text-base-content/50" :disabled="!editor.session.value"
                :title="t('develop.colorBalance.resetAll')" :aria-label="t('develop.colorBalance.resetAll')"
                data-testid="develop-balance-reset-all" @click="reset()"><IconRestore class="w-3 h-3" /></button>
        </div>
        <div v-if="expanded" :id="`${id}-body`" class="px-1 pb-3">
            <div v-if="editor.recipe.value?.sectionVisibility.color === false" class="text-xs text-warning mb-2">
                {{ t('develop.colorBalance.bypassed') }}
            </div>
            <div class="balance-tabs" role="tablist" :aria-label="t('develop.colorBalance.title')">
                <button v-for="(tab, index) in tabs" :key="tab" type="button" role="tab"
                    :id="`${id}-tab-${tab}`" :aria-controls="`${id}-wheels`" :aria-selected="selected === tab" :tabindex="selected === tab ? 0 : -1"
                    :data-testid="`develop-grading-zone-${tab}`" @click="select(tab)" @keydown.stop="tabKey($event, index)">
                    {{ label(tab) }}
                </button>
            </div>
            <div :id="`${id}-wheels`" role="tabpanel" :aria-labelledby="`${id}-tab-${selected}`" :class="{ 'three-way': selected === 'threeWay' }">
                <div v-for="zone in zones" :key="zone" class="balance-zone">
                    <div class="flex items-center justify-between mt-2 text-xs text-base-content/60">
                        <span>{{ label(zone) }}</span>
                        <button type="button" class="btn btn-ghost btn-xs" :disabled="!editor.session.value"
                            :aria-label="t('develop.colorBalance.resetZone', { zone: label(zone) })" :title="t('develop.colorBalance.resetZone', { zone: label(zone) })"
                            :data-testid="`develop-balance-reset-${zone}`" @click="reset(zone)"><IconRestore class="w-3 h-3" /></button>
                    </div>
                    <ColorBalanceWheel :value="grading[zone]" :zone="zone" :label="label(zone)" :master="zone === 'global'" :disabled="!editor.session.value"
                        @live="change(zone, $event, true)" @commit="change(zone, $event, false)" @settle="editor.endEditTransaction()" />
                </div>
            </div>
            <p class="mt-2 text-[10px] text-base-content/40 leading-relaxed">{{ t('develop.colorBalance.help') }}</p>
            <details class="mt-2">
                <summary class="text-xs text-base-content/60 cursor-pointer">{{ t('develop.colorBalance.tonalRanges') }}</summary>
                <DevelopSliderControl v-for="param in (['balance', 'blending'] as const)" :key="param"
                    :label-key="`develop.controls.${param}`" :testid="`grading-${param}`" :min="param === 'balance' ? -100 : 0" :max="100" :step="1" :value="grading[param]" :disabled="!editor.session.value"
                    @live="editor.setParamLive(`colorGrading.${param}`, $event)" @commit="editor.setParam(`colorGrading.${param}`, $event)"
                    @settle="editor.endEditTransaction()" @reset="editor.resetParam(`colorGrading.${param}`)" />
            </details>
        </div>
    </section>
</template>

<style scoped>
.balance-tabs { display: flex; border-bottom: 1px solid #8884; gap: 2px; }
.balance-tabs button { flex: 1; padding: 8px 0; min-width: 0; font-size: 10px; white-space: nowrap; color: inherit; opacity: .65; border-bottom: 2px solid transparent; }
.balance-tabs button[aria-selected='true'] { color: var(--color-primary, #ff7bc8); opacity: 1; border-bottom-color: currentColor; }
.balance-tabs button:focus-visible { outline: 1px solid var(--color-primary, #ff7bc8); }
.color-balance { container-type: inline-size; }
.three-way { display: grid; gap: 10px; }
.three-way .balance-zone { max-width: 250px; width: 100%; margin: auto; }
@container (min-width: 520px) { .three-way { grid-template-columns: repeat(3, minmax(0, 1fr)); } }
</style>
