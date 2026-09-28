<script setup lang="ts">
// Virtual-copy (variant) lifecycle panel for the Develop mode (lap-952 /
// TASK-503). Each virtual copy is its own sidecar
// (`name.ext.lapedit.v-<variantId>.json`) with an independent revision over
// the same immutable source bytes. Create/reset/delete go through explicit
// backend commands; every failure surfaces as visible text and no operation
// ever touches the original file or the other variants.
//
// Switching the editor session to another variant is a later integration
// step; this panel manages the variant inventory and lifecycle only.

import { ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { invoke } from '@tauri-apps/api/core';

const props = defineProps<{
    /** Catalog asset id (file id); null when no file is open. */
    fileId: number | null;
    disabled?: boolean;
    /** When true the panel loads (the section is expanded). */
    active?: boolean;
}>();

const { t } = useI18n();

interface VariantSummary {
    variantId: string;
    revision: number;
    isEdited: boolean;
    isVirtualCopy: boolean;
    contentHash: string;
    sidecarPath: string;
    exists: boolean;
}

const variants = ref<VariantSummary[]>([]);
const busy = ref(false);
const error = ref('');
let listLoadedFor: number | null = null;

async function refresh(fileId: number) {
    error.value = '';
    try {
        const list = await invoke<VariantSummary[]>('develop_list_variants', { assetId: fileId });
        variants.value = Array.isArray(list) ? list : [];
        listLoadedFor = fileId;
    } catch (err) {
        error.value = String(err);
    }
}

watch(
    () => [props.fileId, props.active] as const,
    ([fileId, active]) => {
        if (!fileId || !active) return;
        if (listLoadedFor !== fileId || variants.value.length === 0) {
            void refresh(fileId);
        }
    },
    { immediate: true },
);

async function createCopy() {
    error.value = '';
    if (!props.fileId || props.disabled || busy.value) return;
    busy.value = true;
    try {
        await invoke('develop_create_virtual_copy', {
            assetId: props.fileId,
            fromVariantId: null,
        });
        await refresh(props.fileId);
    } catch (err) {
        error.value = String(err);
    } finally {
        busy.value = false;
    }
}

async function resetVariant(variant: VariantSummary) {
    error.value = '';
    if (!props.fileId || props.disabled || busy.value) return;
    busy.value = true;
    try {
        await invoke('develop_reset_variant', {
            assetId: props.fileId,
            variantId: variant.variantId,
            expectedRevision: variant.revision,
        });
        await refresh(props.fileId);
    } catch (err) {
        error.value = String(err);
    } finally {
        busy.value = false;
    }
}

async function deleteVariant(variant: VariantSummary) {
    error.value = '';
    if (!props.fileId || props.disabled || busy.value || !variant.isVirtualCopy) return;
    busy.value = true;
    try {
        await invoke('develop_delete_variant', {
            assetId: props.fileId,
            variantId: variant.variantId,
        });
        await refresh(props.fileId);
    } catch (err) {
        error.value = String(err);
    } finally {
        busy.value = false;
    }
}
</script>

<template>
    <div class="px-1 pb-2" data-testid="develop-variants-panel">
        <div class="flex items-center gap-2 pt-1">
            <button
                type="button"
                class="btn btn-ghost btn-xs text-base-content/70 hover:text-base-content"
                data-testid="develop-variants-create"
                :disabled="disabled || busy"
                :aria-label="$t('develop.variants.create')"
                @click.stop="createCopy"
            >
                {{ $t('develop.variants.create') }}
            </button>
        </div>

        <div
            v-if="error"
            class="text-[10px] text-error/80 break-words pt-1"
            data-testid="develop-variants-error"
            role="alert"
        >{{ error }}</div>

        <div class="pt-1 flex flex-col gap-0.5">
            <div
                v-for="variant in variants"
                :key="variant.variantId"
                class="flex items-center gap-1 text-xs"
                data-testid="develop-variants-row"
            >
                <span class="font-mono truncate flex-1" :title="variant.sidecarPath">
                    {{ variant.variantId }}
                </span>
                <span class="text-base-content/40 whitespace-nowrap">
                    {{ $t('develop.variants.revision', { revision: variant.revision }) }}
                </span>
                <span
                    class="whitespace-nowrap"
                    :class="variant.isEdited ? 'text-base-content/60' : 'text-base-content/30'"
                >{{
                    variant.isEdited
                        ? $t('develop.variants.edited')
                        : $t('develop.variants.unedited')
                }}</span>
                <button
                    type="button"
                    class="btn btn-ghost btn-xs text-base-content/60 hover:text-base-content"
                    data-testid="develop-variants-reset"
                    :disabled="disabled || busy || !variant.exists"
                    :aria-label="$t('develop.variants.reset')"
                    @click.stop="resetVariant(variant)"
                >{{ $t('develop.variants.reset') }}</button>
                <button
                    v-if="variant.isVirtualCopy"
                    type="button"
                    class="btn btn-ghost btn-xs text-error/70 hover:text-error"
                    data-testid="develop-variants-delete"
                    :disabled="disabled || busy"
                    :aria-label="$t('develop.variants.delete')"
                    @click.stop="deleteVariant(variant)"
                >{{ $t('develop.variants.delete') }}</button>
            </div>
        </div>
    </div>
</template>
