<script setup lang="ts">
// Lens-correction controls for the Develop panel (lap-d52 / TASK-502).
//
// Versioned profile resources with explicit provenance: profiles are locally
// acquired lensfun XML databases imported into the content-addressed store
// (never bundled; distribution terms are an unresolved prerequisite recorded
// in docs/raw-development/provenance.json). Selecting a lens resolves the
// distortion/TCA/vignetting coefficients through the backend engine and
// applies ONE recipe patch carrying the coefficients plus the full
// provenance (maker/model/version/sha256). Capability failures (missing or
// unsupported profiles, unknown lenses) surface as visible errors and apply
// no patch — corrections never silently change the render (spec A7).
// Import version labels are declared by the user at import time; the UI
// never invents one.

import { computed, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';

import {
    type LensDistortionParams,
    type LensProfileRef,
    type LensCorrectionMode,
    type Recipe,
    RECIPE_PARAM_RANGES,
} from '@/composables/useDevelopSession.types';
import DevelopSliderControl from '@/components/develop/DevelopSliderControl.vue';

const props = defineProps<{
    disabled?: boolean;
    /** When true the catalog loads (the panel expands the section). */
    active?: boolean;
    recipe: Recipe | null;
    applyPatch: (patch: Partial<Recipe>, label: string) => void;
}>();

const { t } = useI18n();

interface ProfileSummary {
    id: string;
    lensCount: number;
    cameraCount: number;
}

interface LensNotice {
    kind: string;
    detail: string;
}

const catalog = ref<ProfileSummary[]>([]);
const selectedProfileId = ref('');
const makers = ref<string[]>([]);
const selectedMaker = ref('');
const models = ref<string[]>([]);
const selectedModel = ref('');
const focalLength = ref(50);
const aperture = ref<number | null>(null);
const busy = ref(false);
const error = ref('');
const notices = ref<LensNotice[]>([]);
let catalogLoaded = false;

const provenance = computed<LensProfileRef | null>(() => props.recipe?.lensProfile ?? null);

async function refreshCatalog() {
    try {
        const result = await invoke<ProfileSummary[]>('develop_lens_catalog');
        catalog.value = Array.isArray(result) ? result : [];
        if (catalog.value.length > 0 && !catalog.value.some((p) => p.id === selectedProfileId.value)) {
            selectedProfileId.value = catalog.value[0].id;
            await loadMakers();
        }
    } catch (err) {
        error.value = String(err);
    }
}

async function loadMakers() {
    makers.value = [];
    models.value = [];
    selectedMaker.value = '';
    selectedModel.value = '';
    if (!selectedProfileId.value) return;
    try {
        const result = await invoke<string[]>('develop_lens_makers', { profileId: selectedProfileId.value });
        makers.value = Array.isArray(result) ? result : [];
    } catch (err) {
        error.value = String(err);
    }
}

async function loadModels() {
    models.value = [];
    selectedModel.value = '';
    if (!selectedProfileId.value || !selectedMaker.value) return;
    try {
        const result = await invoke<string[]>('develop_lens_models', {
            profileId: selectedProfileId.value,
            maker: selectedMaker.value,
        });
        models.value = Array.isArray(result) ? result : [];
        if (models.value.length === 1) selectedModel.value = models.value[0];
    } catch (err) {
        error.value = String(err);
    }
}

async function onProfileChange() {
    error.value = '';
    await loadMakers();
}

async function onMakerChange() {
    error.value = '';
    await loadModels();
}

async function importProfiles() {
    error.value = '';
    const selected = await open({
        multiple: true,
        filters: [{ name: 'Lensfun XML', extensions: ['xml'] }],
    });
    if (!selected) return;
    const paths = Array.isArray(selected) ? selected : [selected];
    if (paths.length === 0) return;
    busy.value = true;
    try {
        await invoke('develop_import_lens_profile', {
            paths,
            version: DEFAULT_IMPORT_VERSION,
        });
        await refreshCatalog();
    } catch (err) {
        error.value = String(err);
    } finally {
        busy.value = false;
    }
}

/**
 * The version label recorded at import. Users import local snapshots; this
 * slice records a declared snapshot label rather than pretending to know the
 * upstream version. The label is persisted verbatim in the recipe
 * provenance and audited in docs/raw-development/provenance.json.
 */
const DEFAULT_IMPORT_VERSION = 'local snapshot (unversioned)';

async function applySelection() {
    error.value = '';
    notices.value = [];
    if (!selectedProfileId.value || !selectedMaker.value || !selectedModel.value) return;
    busy.value = true;
    try {
        const selection = await invoke<{
            params: LensDistortionParams;
            notices: LensNotice[];
            profile: LensProfileRef;
        }>('develop_select_lens', {
            profileId: selectedProfileId.value,
            maker: selectedMaker.value,
            model: selectedModel.value,
            version: DEFAULT_IMPORT_VERSION,
            focalLength: focalLength.value,
            aperture: aperture.value ?? null,
            distance: null,
        });
        props.applyPatch(
            {
                lensCorrectionMode: 'manual' as LensCorrectionMode,
                lensMaker: selection.profile.maker,
                lensModel: selection.profile.model,
                lensDistortionParams: selection.params,
                lensProfile: selection.profile,
            },
            'lens-select',
        );
        notices.value = selection.notices;
    } catch (err) {
        error.value = String(err);
    } finally {
        busy.value = false;
    }
}

async function autodetect() {
    error.value = '';
    if (!selectedProfileId.value) return;
    busy.value = true;
    try {
        const match = await invoke<[string, string] | null>('develop_autodetect_lens', {
            profileId: selectedProfileId.value,
            maker: selectedMaker.value || '',
            model: selectedModel.value || '',
        });
        if (match) {
            selectedMaker.value = match[0];
            await loadModels();
            selectedModel.value = match[1];
        } else {
            error.value = t('develop.lens.noMatch');
        }
    } catch (err) {
        error.value = String(err);
    } finally {
        busy.value = false;
    }
}

function setMode(event: Event) {
    props.applyPatch(
        { lensCorrectionMode: (event.target as HTMLSelectElement).value as LensCorrectionMode },
        'lens-mode',
    );
}

const COMPONENT_TOGGLES = [
    { key: 'lensDistortionEnabled' as const, testid: 'develop-lens-distortion-enabled', labelKey: 'develop.lens.distortion' },
    { key: 'lensTcaEnabled' as const, testid: 'develop-lens-tca-enabled', labelKey: 'develop.lens.tca' },
    { key: 'lensVignetteEnabled' as const, testid: 'develop-lens-vignette-enabled', labelKey: 'develop.lens.vignette' },
];

const AMOUNT_SLIDERS = [
    { path: 'lensDistortionAmount', testid: 'lensDistortionAmount', labelKey: 'develop.lens.distortionAmount' },
    { path: 'lensTcaAmount', testid: 'lensTcaAmount', labelKey: 'develop.lens.tcaAmount' },
    { path: 'lensVignetteAmount', testid: 'lensVignetteAmount', labelKey: 'develop.lens.vignetteAmount' },
];

function rangeFor(path: string) {
    const range = RECIPE_PARAM_RANGES[path];
    if (!range) throw new Error(`lens controls: ${path} missing from the generated engine table`);
    return { min: range.min, max: range.max, step: range.step };
}

function amountValue(path: string): number {
    return (props.recipe?.[path as keyof Recipe] as number) ?? 100;
}

function onAmount(path: string, value: number) {
    props.applyPatch({ [path]: value } as Partial<Recipe>, 'lens-amount');
}

function onToggle(key: 'lensDistortionEnabled' | 'lensTcaEnabled' | 'lensVignetteEnabled', event: Event) {
    props.applyPatch(
        { [key]: (event.target as HTMLInputElement).checked } as Partial<Recipe>,
        'lens-toggle',
    );
}

// The catalog loads lazily when the host activates the section: importing
// and listing profile databases touches the backend and must never run as a
// mount side effect (it would also race the host's session open flow).
watch(
    () => props.active,
    (active) => {
        if (active && !catalogLoaded) {
            catalogLoaded = true;
            void refreshCatalog();
        }
    },
    { immediate: true },
);
</script>

<template>
    <div class="px-1 pt-2 space-y-1" data-testid="develop-lens-controls">
        <div class="flex items-center justify-between">
            <span
                class="text-[10px] font-bold uppercase tracking-wide text-base-content/40"
                data-testid="develop-lens-title"
            >{{ $t('develop.lens.title') }}</span>
            <select
                class="select select-xs w-auto"
                data-testid="develop-lens-mode"
                :aria-label="$t('develop.lens.mode')"
                :value="recipe?.lensCorrectionMode || 'manual'"
                :disabled="disabled"
                @change="setMode"
            >
                <option value="manual">{{ $t('develop.lens.modeManual') }}</option>
                <option value="auto">{{ $t('develop.lens.modeAuto') }}</option>
            </select>
        </div>

        <div
            v-if="catalog.length === 0"
            class="px-2 py-1.5 rounded-box bg-warning/10 text-warning text-xs"
            data-testid="develop-lens-no-profiles"
        >
            {{ $t('develop.lens.noProfiles') }}
        </div>

        <template v-else>
            <div class="flex gap-1">
                <select
                    class="select select-xs flex-1"
                    data-testid="develop-lens-profile"
                    :aria-label="$t('develop.lens.profile')"
                    v-model="selectedProfileId"
                    :disabled="disabled || busy"
                    @change="onProfileChange"
                >
                    <option v-for="profile in catalog" :key="profile.id" :value="profile.id">
                        {{ $t('develop.lens.profileOption', { lenses: profile.lensCount }) }}
                    </option>
                </select>
                <button
                    type="button"
                    class="btn btn-ghost btn-xs"
                    data-testid="develop-lens-import"
                    :disabled="disabled || busy"
                    @click="importProfiles"
                >{{ $t('develop.lens.import') }}</button>
            </div>

            <div class="flex gap-1">
                <select
                    class="select select-xs flex-1"
                    data-testid="develop-lens-maker"
                    :aria-label="$t('develop.lens.maker')"
                    v-model="selectedMaker"
                    :disabled="disabled || busy"
                    @change="onMakerChange"
                >
                    <option v-for="maker in makers" :key="maker" :value="maker">{{ maker }}</option>
                </select>
                <select
                    class="select select-xs flex-1"
                    data-testid="develop-lens-model"
                    :aria-label="$t('develop.lens.model')"
                    v-model="selectedModel"
                    :disabled="disabled || busy"
                    @change="() => {}"
                >
                    <option v-for="model in models" :key="model" :value="model">{{ model }}</option>
                </select>
            </div>

            <div class="flex items-center gap-1">
                <label class="text-xs text-base-content/50" for="develop-lens-focal">
                    {{ $t('develop.lens.focal') }}
                </label>
                <input
                    id="develop-lens-focal"
                    type="number"
                    class="input input-xs w-20"
                    data-testid="develop-lens-focal"
                    v-model.number="focalLength"
                    min="1"
                    max="1200"
                    step="1"
                    :disabled="disabled || busy"
                />
                <button
                    type="button"
                    class="btn btn-ghost btn-xs ml-auto"
                    data-testid="develop-lens-apply"
                    :disabled="disabled || busy || !selectedModel"
                    @click="applySelection"
                >{{ $t('develop.lens.apply') }}</button>
            </div>
        </template>

        <div
            v-if="error"
            class="px-2 py-1.5 rounded-box bg-error/10 text-error text-xs break-words"
            data-testid="develop-lens-error"
        >{{ error }}</div>

        <div
            v-if="notices.length > 0"
            class="px-2 py-1.5 rounded-box bg-warning/10 text-warning text-xs break-words space-y-0.5"
            data-testid="develop-lens-notices"
        >
            <div v-for="notice in notices" :key="notice.kind + notice.detail">{{ notice.detail }}</div>
        </div>

        <div
            v-if="provenance"
            class="text-[10px] text-base-content/40 break-words"
            data-testid="develop-lens-provenance"
            :title="provenance.sha256"
        >
            {{ $t('develop.lens.provenance', {
                maker: provenance.maker,
                model: provenance.model,
                version: provenance.version,
                hash: provenance.sha256.slice(0, 8),
            }) }}
        </div>

        <template v-if="recipe">
            <label
                v-for="toggle in COMPONENT_TOGGLES"
                :key="toggle.key"
                class="flex items-center gap-2 text-xs text-base-content/70"
            >
                <input
                    type="checkbox"
                    class="checkbox checkbox-xs checkbox-primary"
                    :data-testid="toggle.testid"
                    :checked="recipe[toggle.key]"
                    :disabled="disabled"
                    @change="onToggle(toggle.key, $event)"
                />
                {{ $t(toggle.labelKey) }}
            </label>

            <DevelopSliderControl
                v-for="slider in AMOUNT_SLIDERS"
                :key="slider.path"
                :label-key="slider.labelKey"
                :min="rangeFor(slider.path).min"
                :max="rangeFor(slider.path).max"
                :step="rangeFor(slider.path).step"
                :value="amountValue(slider.path)"
                :testid="slider.testid"
                :disabled="disabled"
                @live="(value: number) => onAmount(slider.path, value)"
                @settle="() => {}"
                @commit="(value: number) => onAmount(slider.path, value)"
                @reset="() => onAmount(slider.path, 100)"
            />
        </template>
    </div>
</template>
