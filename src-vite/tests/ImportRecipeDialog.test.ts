import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount, flushPromises } from '@vue/test-utils';

// Contract tests for the explicit .rrdata import dialog (lap-5c2 / TASK-404;
// managed continuation of lap-002.4).
//
// Governing contract: docs/raw-development/spec.md ("Persistence and
// compatibility", A9). The UI half pinned here:
//   - the preview/validate step happens first and performs no writes;
//   - limitations of the pinned engine slice are shown BEFORE the user can
//     confirm, and a non-faithful import says so explicitly;
//   - applying is a separate explicit action (the durable commit itself is
//     the editor's `applyImportedRecipe`);
//   - backend rejections (corrupt/future/missing documents) are visible and
//     never fall back to defaults.

const invokeMock = vi.fn();

vi.mock('@tauri-apps/api/core', () => ({
    invoke: (...args: unknown[]) => invokeMock(...args),
}));

const openMock = vi.fn();

vi.mock('@tauri-apps/plugin-dialog', () => ({
    open: (...args: unknown[]) => openMock(...args),
}));

import ImportRecipeDialog from '@/components/develop/ImportRecipeDialog.vue';
import {
    DEFAULT_RECIPE,
    RECIPE_SCHEMA_VERSION,
    type OpenedEditSession,
} from '@/composables/useDevelopSession.types';

function openedSession(): OpenedEditSession {
    return {
        sessionId: 11,
        assetId: '42',
        variantId: 'default',
        revision: 2,
        dimensions: [6000, 4000],
        sourceFingerprint: 'f'.repeat(64),
        envelope: {
            schemaVersion: RECIPE_SCHEMA_VERSION,
            engineVersion: 'lap/0.3.2/rapidraw-edit-model/0.1.0',
            assetId: '42',
            variantId: 'default',
            revision: 2,
            sourceFingerprint: 'f'.repeat(64),
            decode: {},
            recipe: structuredClone(DEFAULT_RECIPE),
            resources: {},
            unsupported: {},
        },
    } as OpenedEditSession;
}

function importOutcome(overrides: Record<string, unknown> = {}) {
    return {
        envelope: {
            recipe: { ...structuredClone(DEFAULT_RECIPE), exposure: 0.5 },
            unsupported: { 'legacyMetadata.rating': 4 },
        },
        limitations: [],
        preservedKeys: ['legacyMetadata.rating'],
        excludedUiFields: ['showClipping'],
        migrationApplied: ['legacy-rrdata-v0'],
        original: { sha256: 'a'.repeat(64), byteLen: 4334 },
        faithfulSubset: true,
        ...overrides,
    };
}

const applyImportedMock = vi.fn();

async function mountDialog(session: OpenedEditSession | null = openedSession()) {
    return mount(ImportRecipeDialog, {
        props: {
            session,
            applyImported: applyImportedMock,
        },
    });
}

async function runPreview(
    wrapper: Awaited<ReturnType<typeof mountDialog>>,
    outcome: Record<string, unknown> | Error,
) {
    openMock.mockResolvedValueOnce('C:/photos/photo.CR2.rrdata');
    if (outcome instanceof Error) {
        invokeMock.mockRejectedValueOnce(outcome);
    } else {
        invokeMock.mockResolvedValueOnce(outcome);
    }
    await wrapper.get('[data-testid="develop-import-browse"]').trigger('click');
    await flushPromises();
}

describe('ImportRecipeDialog', () => {
    beforeEach(() => {
        invokeMock.mockReset();
        openMock.mockReset();
        applyImportedMock.mockReset();
    });

    afterEach(() => {
        vi.restoreAllMocks();
    });

    it('renders nothing without an open develop session', () => {
        const wrapper = mount(ImportRecipeDialog, {
            props: { session: null, applyImported: applyImportedMock },
        });
        expect(wrapper.find('[data-testid="develop-import-dialog"]').exists()).toBe(false);
    });

    it('disables confirming until a document has been previewed', async () => {
        const wrapper = await mountDialog();
        const confirm = wrapper.get('[data-testid="develop-import-confirm"]')
            .element as HTMLButtonElement;
        expect(confirm.disabled).toBe(true);
    });

    it('previews with the session identity and applies only on explicit confirm', async () => {
        const wrapper = await mountDialog();
        await runPreview(wrapper, importOutcome());

        expect(invokeMock).toHaveBeenCalledWith(
            'develop_import_rrdata',
            expect.objectContaining({
                rrdataPath: 'C:/photos/photo.CR2.rrdata',
                assetId: 42,
                variantId: 'default',
                sourceFingerprint: 'f'.repeat(64),
                sourceWidth: 6000,
                sourceHeight: 4000,
            }),
        );
        expect(wrapper.find('[data-testid="develop-import-faithful"]').exists()).toBe(true);
        expect(
            (wrapper.get('[data-testid="develop-import-confirm"]').element as HTMLButtonElement)
                .disabled,
        ).toBe(false);

        applyImportedMock.mockResolvedValueOnce(true);
        await wrapper.get('[data-testid="develop-import-confirm"]').trigger('click');
        await flushPromises();

        expect(applyImportedMock).toHaveBeenCalledTimes(1);
        const [recipe, unsupported] = applyImportedMock.mock.calls[0];
        expect(recipe.exposure).toBe(0.5);
        expect(unsupported['legacyMetadata.rating']).toBe(4);
        expect(wrapper.emitted('applied')).toBeTruthy();
        expect(
            wrapper.get('[data-testid="develop-import-status"]').text(),
        ).toContain('committed');
    });

    it('names every limitation and blocks the faithful claim', async () => {
        const wrapper = await mountDialog();
        await runPreview(
            wrapper,
            importOutcome({
                faithfulSubset: false,
                limitations: [
                    { kind: 'lens-blur', detail: 'lens blur stays unrendered' },
                    { kind: 'missing-resource', detail: 'LUT file missing: look.cube' },
                    { kind: 'lut-resource', detail: 'LUT not applied by this slice' },
                    { kind: 'masks', detail: '1 visible mask' },
                    { kind: 'ai-patches', detail: 'generative edits preserved' },
                ],
            }),
        );

        expect(wrapper.find('[data-testid="develop-import-faithful"]').exists()).toBe(false);
        expect(wrapper.get('[data-testid="develop-import-not-faithful"]').exists()).toBe(true);
        for (const kind of ['lens-blur', 'missing-resource', 'lut-resource', 'masks', 'ai-patches']) {
            expect(
                wrapper.find(`[data-testid="develop-import-limitation-${kind}"]`).exists(),
            ).toBe(true);
        }
        expect(wrapper.get('[data-testid="develop-import-preserved"]').text()).toContain(
            'legacyMetadata.rating',
        );

        // A non-faithful import still applies when explicitly confirmed —
        // with the limitations having been shown first.
        applyImportedMock.mockResolvedValueOnce(true);
        await wrapper.get('[data-testid="develop-import-confirm"]').trigger('click');
        await flushPromises();
        expect(applyImportedMock).toHaveBeenCalledTimes(1);
    });

    it('surfaces backend rejections visibly and never applies', async () => {
        const wrapper = await mountDialog();
        await runPreview(
            wrapper,
            new Error(
                'rrdata file declares source schema version 99, newer than the highest supported version 1',
            ),
        );

        expect(wrapper.get('[data-testid="develop-import-error"]').text()).toContain('version 99');
        expect(
            (wrapper.get('[data-testid="develop-import-confirm"]').element as HTMLButtonElement)
                .disabled,
        ).toBe(true);
        expect(applyImportedMock).not.toHaveBeenCalled();
    });

    it('shows a visible failure when the apply step rejects', async () => {
        const wrapper = await mountDialog();
        await runPreview(wrapper, importOutcome());

        applyImportedMock.mockRejectedValueOnce(new Error('revision-conflict: expected 2'));
        await wrapper.get('[data-testid="develop-import-confirm"]').trigger('click');
        await flushPromises();

        expect(wrapper.get('[data-testid="develop-import-error"]').text()).toContain(
            'revision-conflict',
        );
        expect(wrapper.emitted('applied')).toBeFalsy();
    });
});
