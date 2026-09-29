<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useDevelopEditor } from '@/composables/useDevelopEditor';
import { DEFAULT_RECIPE, type VignettingMethod } from '@/composables/useDevelopSession.types';
import DevelopSection from './DevelopSection.vue';
import DevelopSliderControl from './DevelopSliderControl.vue';
const { t } = useI18n();
const editor = useDevelopEditor();
const expanded = ref(false);
const settings = computed(() => editor.recipe.value?.vignetting ?? DEFAULT_RECIPE.vignetting);
const methods: VignettingMethod[] = ['ellipticOnCrop', 'circularOnCrop', 'circular'];
function amount(value: number, live: boolean) {
    if (!Number.isFinite(value)) return;
    const patch = { vignetting: { ...settings.value, amount: Math.max(-4, Math.min(4, value)) } };
    if (live) editor.applyRecipePatchLive(patch, 'vignetting amount');
    else editor.applyRecipePatch(patch, 'vignetting amount');
}
function method(event: Event) {
    const value = (event.target as HTMLSelectElement).value as VignettingMethod;
    if (!methods.includes(value)) return;
    editor.endEditTransaction();
    editor.applyRecipePatch({ vignetting: { ...settings.value, method: value } }, 'vignetting method');
}
function enable(enabled: boolean) {
    editor.endEditTransaction();
    editor.applyRecipePatch({ vignetting: { ...settings.value, enabled } }, 'bypass vignetting');
}
function reset() {
    editor.endEditTransaction();
    editor.applyRecipePatch({ vignetting: { ...DEFAULT_RECIPE.vignetting } }, 'reset vignetting');
}
function toggle() { editor.endEditTransaction(); expanded.value = !expanded.value; }
onBeforeUnmount(() => editor.endEditTransaction());
</script>
<template>
    <DevelopSection section-id="vignetting" label-key="develop.vignetting.title"
        :visible="settings.enabled" :expanded="expanded"
        @toggle-expand="toggle" @toggle-visible="enable" @reset="reset">
        <DevelopSliderControl label-key="develop.vignetting.amount" testid="vignetting-amount"
            :value="settings.amount" :min="-4" :max="4" :step="0.05" :disabled="!editor.session.value"
            @live="amount($event, true)" @commit="amount($event, false)"
            @settle="editor.endEditTransaction()" @reset="amount(0, false)" />
        <label class="flex items-center gap-3 px-1 py-2 text-xs">
            <span>{{ t('develop.vignetting.method') }}</span>
            <select class="select select-xs flex-1 min-w-0" :value="settings.method"
                :disabled="!editor.session.value" :aria-label="t('develop.vignetting.method')"
                data-testid="vignetting-method" @change="method">
                <option v-for="item in methods" :key="item" :value="item">{{ t(`develop.vignetting.${item}`) }}</option>
            </select>
        </label>
    </DevelopSection>
</template>
