<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';

import {
    useDevelopEditor,
} from '@/composables/useDevelopEditor';
import {
    type Recipe,
    type SectionId,
    type ToneMapper,
} from '@/composables/useDevelopSession.types';
import {
    BASIC_TONE_MAPPER_CONTROL,
    DEVELOP_CONTROL_GROUPS,
    DEVELOP_SECTIONS,
    getRecipeValue,
    type ControlGroup,
    type ParamDescriptor,
} from '@/components/develop/controls';
import DevelopSliderControl from '@/components/develop/DevelopSliderControl.vue';
import DevelopSection from '@/components/develop/DevelopSection.vue';
import DevelopCurveEditor from '@/components/develop/DevelopCurveEditor.vue';
import DevelopMasksSection from '@/components/develop/masks/MasksSection.vue';
import LensControls from '@/components/develop/LensControls.vue';
import VariantsPanel from '@/components/develop/VariantsPanel.vue';
import ImportRecipeDialog from '@/components/develop/ImportRecipeDialog.vue';
import ImageHistogram from '@/components/ImageHistogram.vue';
import TButton from '@/components/TButton.vue';
import { IconClose } from '@/common/icons';

/**
 * Native Develop panel (lap-0e9 / TASK-303; global controls lap-adc /
 * TASK-401; spec A10).
 *
 * All first-release global controls (tone, white balance, color/HSL, curves,
 * detail, grain/vignette and section bypass) are generated from the engine
 * descriptors (RECIPE_PARAM_RANGES + model-validated nested bounds). Every
 * meaningful action forms one undo transaction (session-scoped, never
 * persisted); edits go through the useDevelopEditor orchestration: debounced
 * recipe commits with saving/saved/conflict/failed status, awaited flush on
 * navigation/close, retained dirty state with explicit retry, per-control /
 * section / global reset, and an explicit original comparison toggle.
 * The histogram is fed from the displayed preview generation. This panel
 * never writes pixels to the source file.
 */

const props = defineProps<{
    file: Record<string, any> | null;
}>();

const emit = defineEmits<{
    close: [];
}>();

const { t } = useI18n();
const develop = useDevelopEditor();

const expandedSections = ref<Record<SectionId, boolean>>({
    basic: true,
    curves: false,
    color: false,
    details: false,
    effects: false,
});
const selectedHslChannel = ref<string>('reds');
const selectedGradingZone = ref<string>('global');
const lensExpanded = ref(false);
const variantsExpanded = ref(false);
const importDialogOpen = ref(false);

const groupsBySection = computed<Record<SectionId, ControlGroup[]>>(() => {
    const map = {
        basic: [],
        curves: [],
        color: [],
        details: [],
        effects: [],
    } as Record<SectionId, ControlGroup[]>;
    for (const group of DEVELOP_CONTROL_GROUPS) {
        map[group.section].push(group);
    }
    return map;
});

function testidFor(path: string): string {
    return path
        .replace(/\./g, '-')
        .replace(/^colorGrading-/, 'grading-')
        .replace(/^colorCalibration-/, 'calibration-');
}

function controlValue(path: string): number {
    const recipe = develop.recipe.value;
    if (!recipe) return 0;
    return getRecipeValue(recipe, path) ?? 0;
}

function sliderProps(param: ParamDescriptor) {
    return {
        labelKey: param.labelKey,
        min: param.range.min,
        max: param.range.max,
        step: param.range.step,
        value: controlValue(param.path),
        testid: testidFor(param.path),
    };
}

function onSliderLive(path: string, value: number) {
    develop.setParamLive(path, value);
}

function onSliderSettle() {
    develop.endEditTransaction();
}

function onSliderCommit(path: string, value: number) {
    develop.setParam(path, value);
}

function onSliderReset(path: string) {
    develop.resetParam(path);
}

function hslChannelParams(channel: string): ParamDescriptor[] {
    const group = groupsBySection.value.color.find((g) => g.id === 'hsl');
    return group ? group.params.filter((p) => p.path.startsWith(`hsl.${channel}.`)) : [];
}

function gradingZoneParams(zone: string): ParamDescriptor[] {
    const group = groupsBySection.value.color.find((g) => g.id === 'colorGrading');
    return group ? group.params.filter((p) => p.path.startsWith(`colorGrading.${zone}.`)) : [];
}

function gradingCommonParams(): ParamDescriptor[] {
    const group = groupsBySection.value.color.find((g) => g.id === 'colorGrading');
    return group
        ? group.params.filter((p) => !/^colorGrading\.(global|shadows|midtones|highlights)\./.test(p.path))
        : [];
}

function onToggleExpand(section: SectionId) {
    expandedSections.value[section] = !expandedSections.value[section];
}

function onToggleVisible(section: SectionId, visible: boolean) {
    develop.setSectionVisible(section, visible);
}

function onResetSection(section: SectionId) {
    develop.resetSection(section);
}

function sectionVisible(section: SectionId): boolean {
    return develop.recipe.value?.sectionVisibility[section] ?? true;
}

function resetAll() {
    develop.resetAll();
}

function undo() {
    develop.undo();
}

function redo() {
    develop.redo();
}

/**
 * Explicit rrdata import (lap-5c2): the dialog validates and reports first;
 * applying goes through the editor's immediate durable commit.
 */
async function applyImported(
    recipe: Recipe,
    unsupported: Record<string, unknown>,
): Promise<boolean> {
    return develop.applyImportedRecipe(recipe, unsupported);
}

function toggleOriginal() {
    develop.showOriginal.value = !develop.showOriginal.value;
}

function onToneMapperChange(event: Event) {
    develop.setToneMapper((event.target as HTMLSelectElement).value as ToneMapper);
}

/**
 * The histogram reflects exactly the displayed generation: while the
 * rendered preview is shown it is fed from the displayed preview frame
 * (geometry and adjustments are already baked in); while the original
 * comparison is active the histogram source is the untouched original
 * thumbnail and no renderer input is passed.
 */
const histogramPixels = computed(() => {
    if (develop.showOriginal.value) return null;
    const preview = develop.preview.value;
    if (!preview || !(preview.bytes instanceof ArrayBuffer) || preview.bytes.byteLength === 0) {
        return null;
    }
    return {
        data: new Uint8ClampedArray(preview.bytes),
        width: preview.width,
        height: preview.height,
    };
});

const histogramSource = computed(() => (develop.showOriginal.value ? String(props.file?.thumbnail || '') : ''));

const statusLabel = computed(() => {
    const state = develop.saveState.value;
    if (state === 'idle' || state === 'pending') {
        return state === 'pending' ? t('develop.save.pending') : t('develop.save.idle');
    }
    return t(`develop.save.${state}`);
});

const showRetry = computed(
    () => develop.saveState.value === 'failed' || develop.saveState.value === 'conflict',
);

async function retryCommit() {
    await develop.retry();
}

function requestClose() {
    emit('close');
}

watch(
    () => Number(props.file?.id || 0),
    async (fileId) => {
        if (!fileId) return;
        try {
            await develop.openAsset({ id: fileId });
        } catch {
            // Open failures are surfaced through develop.openError.
        }
    },
    { immediate: true },
);

onBeforeUnmount(() => {
    // Awaited commits happen through the panel-mode switching paths; the
    // composable keeps per-asset state if this panel unmounts unexpectedly.
    void develop.flush().catch(() => {});
});
</script>

<template>
    <div class="w-full h-full rounded-box bg-base-200 flex flex-col overflow-hidden" data-testid="develop-panel">
        <!-- Header: title, session undo/redo, close -->
        <div class="my-2 px-2 flex items-center w-full shrink-0">
            <div class="flex-1 pl-1">
                <span class="text-sm font-semibold text-primary/70">{{ $t('develop.title') }}</span>
            </div>
            <div class="flex items-center gap-1">
                <button
                    type="button"
                    class="btn btn-ghost btn-xs text-base-content/60 hover:text-base-content disabled:text-base-content/30"
                    data-testid="develop-import"
                    :title="$t('develop.importHint')"
                    :aria-label="$t('develop.import')"
                    :disabled="!develop.session.value"
                    @click.stop="importDialogOpen = true"
                >{{ $t('develop.import') }}</button>
                <button
                    type="button"
                    class="btn btn-ghost btn-xs text-base-content/60 hover:text-base-content disabled:text-base-content/30"
                    data-testid="develop-undo"
                    :disabled="!develop.canUndo.value"
                    :title="$t('develop.historyHint')"
                    :aria-label="$t('develop.undo')"
                    @click.stop="undo"
                >{{ $t('develop.undo') }}</button>
                <button
                    type="button"
                    class="btn btn-ghost btn-xs text-base-content/60 hover:text-base-content disabled:text-base-content/30"
                    data-testid="develop-redo"
                    :disabled="!develop.canRedo.value"
                    :title="$t('develop.historyHint')"
                    :aria-label="$t('develop.redo')"
                    @click.stop="redo"
                >{{ $t('develop.redo') }}</button>
                <TButton
                    :icon="IconClose"
                    :tooltip="$t('msgbox.close')"
                    :buttonSize="'small'"
                    @click.stop="requestClose"
                />
            </div>
        </div>

        <div v-if="file" class="mb-2 px-2 flex-1 overflow-y-auto overflow-x-hidden flex flex-col gap-1">
            <div
                v-if="develop.openError.value"
                class="px-2 py-1.5 rounded-box bg-error/10 text-error text-xs break-words"
                data-testid="develop-open-error"
            >{{ develop.openError.value }}</div>

            <!-- Histogram of the displayed generation -->
            <div class="px-1 pt-1">
                <div class="text-[10px] uppercase tracking-wide text-base-content/40 mb-0.5">
                    {{ $t('develop.histogram') }}
                </div>
                <ImageHistogram
                    :source="histogramSource"
                    :renderer-pixels="histogramPixels"
                />
            </div>

            <!-- First-release global sections -->
            <DevelopSection
                v-for="section in DEVELOP_SECTIONS"
                :key="section.id"
                :section-id="section.id"
                :label-key="section.labelKey"
                :visible="sectionVisible(section.id)"
                :expanded="expandedSections[section.id]"
                @toggle-expand="onToggleExpand(section.id)"
                @toggle-visible="(value) => onToggleVisible(section.id, value)"
                @reset="onResetSection(section.id)"
            >
                <!-- Curves section: dedicated editor -->
                <DevelopCurveEditor v-if="section.id === 'curves'" />

                <!-- Scalar sections: control groups -->
                <div v-else class="space-y-0.5">
                    <template v-for="group in groupsBySection[section.id]" :key="group.id">
                        <!-- HSL: channel selector + selected channel's components -->
                        <div v-if="group.id === 'hsl'" class="px-1 pt-2">
                            <div class="text-[10px] font-bold uppercase tracking-wide text-base-content/40 mb-1">
                                {{ $t(group.labelKey) }}
                            </div>
                            <div class="flex items-center gap-0.5 mb-1">
                                <button
                                    v-for="channel in ['reds','oranges','yellows','greens','aquas','blues','purples','magentas']"
                                    :key="channel"
                                    type="button"
                                    class="btn btn-ghost btn-xs capitalize"
                                    :class="selectedHslChannel === channel ? 'text-primary' : 'text-base-content/50'"
                                    :data-testid="`develop-hsl-channel-${channel}`"
                                    :aria-label="$t('develop.hslChannels.' + channel)"
                                    :aria-pressed="selectedHslChannel === channel ? 'true' : 'false'"
                                    @click.stop="selectedHslChannel = channel"
                                >{{ $t('develop.hslChannels.' + channel) }}</button>
                            </div>
                            <DevelopSliderControl
                                v-for="param in hslChannelParams(selectedHslChannel)"
                                :key="param.path"
                                v-bind="sliderProps(param)"
                                @live="(value) => onSliderLive(param.path, value)"
                                @settle="onSliderSettle"
                                @commit="(value) => onSliderCommit(param.path, value)"
                                @reset="onSliderReset(param.path)"
                            />
                        </div>

                        <!-- Color grading: zone selector + selected zone + balance/blending -->
                        <div v-else-if="group.id === 'colorGrading'" class="px-1 pt-2">
                            <div class="text-[10px] font-bold uppercase tracking-wide text-base-content/40 mb-1">
                                {{ $t(group.labelKey) }}
                            </div>
                            <div class="flex items-center gap-0.5 mb-1">
                                <button
                                    v-for="zone in ['global','shadows','midtones','highlights']"
                                    :key="zone"
                                    type="button"
                                    class="btn btn-ghost btn-xs capitalize"
                                    :class="selectedGradingZone === zone ? 'text-primary' : 'text-base-content/50'"
                                    :data-testid="`develop-grading-zone-${zone}`"
                                    :aria-label="$t('develop.gradingZones.' + zone)"
                                    :aria-pressed="selectedGradingZone === zone ? 'true' : 'false'"
                                    @click.stop="selectedGradingZone = zone"
                                >{{ $t('develop.gradingZones.' + zone) }}</button>
                            </div>
                            <DevelopSliderControl
                                v-for="param in gradingZoneParams(selectedGradingZone)"
                                :key="param.path"
                                v-bind="sliderProps(param)"
                                @live="(value) => onSliderLive(param.path, value)"
                                @settle="onSliderSettle"
                                @commit="(value) => onSliderCommit(param.path, value)"
                                @reset="onSliderReset(param.path)"
                            />
                            <DevelopSliderControl
                                v-for="param in gradingCommonParams()"
                                :key="param.path"
                                v-bind="sliderProps(param)"
                                @live="(value) => onSliderLive(param.path, value)"
                                @settle="onSliderSettle"
                                @commit="(value) => onSliderCommit(param.path, value)"
                                @reset="onSliderReset(param.path)"
                            />
                        </div>

                        <!-- Regular slider groups -->
                        <div v-else>
                            <div class="px-1 pt-2 text-[10px] font-bold uppercase tracking-wide text-base-content/40">
                                {{ $t(group.labelKey) }}
                            </div>
                            <DevelopSliderControl
                                v-for="param in group.params"
                                :key="param.path"
                                v-bind="sliderProps(param)"
                                @live="(value) => onSliderLive(param.path, value)"
                                @settle="onSliderSettle"
                                @commit="(value) => onSliderCommit(param.path, value)"
                                @reset="onSliderReset(param.path)"
                            />
                        </div>

                        <!-- Tone mapper select closes the basic section -->
                        <div v-if="section.id === 'basic' && group.id === 'tone'" class="px-1 py-2">
                            <label class="text-xs font-bold uppercase tracking-wide text-base-content/30 block mb-1">
                                {{ $t(BASIC_TONE_MAPPER_CONTROL.labelKey) }}
                            </label>
                            <select
                                class="select select-xs w-full"
                                data-testid="develop-tonemapper"
                                :aria-label="$t(BASIC_TONE_MAPPER_CONTROL.labelKey)"
                                :value="develop.recipe.value?.toneMapper || 'basic'"
                                @change="onToneMapperChange"
                            >
                                <option
                                    v-for="option in BASIC_TONE_MAPPER_CONTROL.options"
                                    :key="option.value"
                                    :value="option.value"
                                >{{ $t(option.labelKey) }}</option>
                            </select>
                        </div>
                    </template>
                </div>
            </DevelopSection>

            <!-- Local masks (lap-78d): native brush/linear/radial tools;
                 gestures run on the central preview overlay -->
            <div class="border-t border-base-content/5" data-testid="develop-section-masks">
                <div class="px-1 pt-2 pb-1">
                    <span class="font-bold uppercase text-[11px] tracking-wide text-base-content/40">
                        {{ $t('develop.masks.title') }}
                    </span>
                </div>
                <div class="pb-1">
                    <DevelopMasksSection :disabled="!develop.session.value" />
                </div>
            </div>

            <!-- Lens correction (lap-d52): versioned profile resources with
                 explicit provenance; missing/unsupported profiles surface as
                 visible capability errors, never silent correction changes -->
            <div class="border-t border-base-content/5" data-testid="develop-section-lens">
                <div class="px-1 pt-2 pb-1">
                    <button
                        type="button"
                        class="font-bold uppercase text-[11px] tracking-wide text-base-content/40 hover:text-base-content/70"
                        data-testid="develop-lens-section-toggle"
                        :aria-expanded="lensExpanded ? 'true' : 'false'"
                        @click.stop="lensExpanded = !lensExpanded"
                    >
                        {{ $t('develop.lens.title') }}
                    </button>
                </div>
                <LensControls
                    v-show="lensExpanded"
                    :active="lensExpanded"
                    :disabled="!develop.session.value"
                    :recipe="develop.recipe.value"
                    :apply-patch="(patch, label) => develop.applyRecipePatch(patch, label)"
                />
            </div>

            <!-- Virtual copies (lap-952): per-variant sidecars with
                 independent revisions over the same immutable source;
                 create/reset/delete are explicit backend operations -->
            <div class="border-t border-base-content/5" data-testid="develop-section-variants">
                <div class="px-1 pt-2 pb-1">
                    <button
                        type="button"
                        class="font-bold uppercase text-[11px] tracking-wide text-base-content/40 hover:text-base-content/70"
                        data-testid="develop-variants-section-toggle"
                        :aria-expanded="variantsExpanded ? 'true' : 'false'"
                        @click.stop="variantsExpanded = !variantsExpanded"
                    >
                        {{ $t('develop.variants.title') }}
                    </button>
                </div>
                <VariantsPanel
                    v-show="variantsExpanded"
                    :active="variantsExpanded"
                    :disabled="!develop.session.value || !file?.id"
                    :file-id="file?.id ?? null"
                />
            </div>

            <!-- Reset all -->
            <div class="border-t border-base-content/5 px-1 py-2">
                <button
                    type="button"
                    class="btn btn-ghost btn-xs w-full text-base-content/60 hover:text-base-content"
                    data-testid="develop-reset-all"
                    @click.stop="resetAll"
                >
                    {{ $t('develop.resetAll') }}
                </button>
            </div>
        </div>

        <!-- Footer: original comparison + save status -->
        <div
            v-if="file"
            class="px-2 pb-2 pt-1 border-t border-base-content/5 shrink-0 flex flex-col gap-1"
        >
            <button
                type="button"
                class="btn btn-ghost btn-xs justify-start text-base-content/70 hover:text-base-content"
                data-testid="develop-view-original"
                :aria-pressed="develop.showOriginal.value ? 'true' : 'false'"
                @click.stop="toggleOriginal"
            >
                {{ develop.showOriginal.value ? $t('develop.viewDeveloped') : $t('develop.viewOriginal') }}
            </button>

            <div class="flex items-center gap-2">
                <span
                    class="text-xs flex-1"
                    :class="{
                        'text-base-content/50': develop.saveState.value === 'idle',
                        'text-warning': develop.saveState.value === 'pending' || develop.saveState.value === 'saving',
                        'text-success': develop.saveState.value === 'saved',
                        'text-error': develop.saveState.value === 'failed' || develop.saveState.value === 'conflict',
                    }"
                    data-testid="develop-save-status"
                    :title="develop.lastError.value || ''"
                >{{ statusLabel }}</span>
                <button
                    v-if="showRetry"
                    type="button"
                    class="btn btn-xs btn-warning"
                    data-testid="develop-retry"
                    @click.stop="retryCommit"
                >{{ $t('develop.retry') }}</button>
            </div>
            <div
                v-if="develop.lastError.value && showRetry"
                class="text-[10px] text-error/80 break-words"
                data-testid="develop-save-error"
            >{{ develop.lastError.value }}</div>
        </div>

        <!-- Explicit .rrdata compatibility import (lap-5c2) -->
        <ImportRecipeDialog
            v-if="importDialogOpen"
            :session="develop.session.value as any"
            :apply-imported="applyImported"
            @applied="importDialogOpen = false"
            @close="importDialogOpen = false"
        />
    </div>
</template>
