import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushPromises, mount } from "@vue/test-utils";
import { createI18n } from "vue-i18n";

// Component regressions for virtual copies (lap-952 / TASK-503). Spec refs
// A8/A9/A10: variant identities are listed with their revisions, create/reset/
// delete go through the explicit backend lifecycle commands, failures are
// visible, and the primary variant is never deletable from the UI.

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import VariantsPanel from "@/components/develop/VariantsPanel.vue";
import enMessages from "@/locales/en.json";

function makeI18n() {
  return createI18n({
    legacy: false,
    locale: "en",
    fallbackLocale: "en",
    messages: { en: enMessages as Record<string, unknown> },
  });
}

const DEFAULT_VARIANT = {
  variantId: "default",
  revision: 3,
  isEdited: true,
  isVirtualCopy: false,
  contentHash: "a".repeat(64),
  sidecarPath: "C:/lib/photo.arw.lapedit.json",
  exists: true,
};

const VIRTUAL_COPY = {
  variantId: "vc-abc123",
  revision: 1,
  isEdited: true,
  isVirtualCopy: true,
  contentHash: "b".repeat(64),
  sidecarPath: "C:/lib/photo.arw.lapedit.v-vc-abc123.json",
  exists: true,
};

function mountPanel(fileId: number | null = 42) {
  return mount(VariantsPanel, {
    props: {
      fileId,
      disabled: false,
      active: true,
    },
    global: {
      plugins: [makeI18n()],
    },
  });
}

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockImplementation((command: string) => {
    if (command === "develop_list_variants")
      return Promise.resolve([DEFAULT_VARIANT, VIRTUAL_COPY]);
    if (command === "develop_create_virtual_copy")
      return Promise.resolve({ ...VIRTUAL_COPY, variantId: "vc-newcopy" });
    if (command === "develop_reset_variant")
      return Promise.resolve({
        variantId: VIRTUAL_COPY.variantId,
        revision: 2,
        contentHash: "c".repeat(64),
        sidecarPath: VIRTUAL_COPY.sidecarPath,
      });
    if (command === "develop_delete_variant") return Promise.resolve(undefined);
    return Promise.resolve(undefined);
  });
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("VariantsPanel", () => {
  it("lists the default variant first with revisions and lifecycle facts", async () => {
    const wrapper = mountPanel();
    await flushPromises();

    expect(invokeMock).toHaveBeenCalledWith("develop_list_variants", { assetId: 42 });
    const rows = wrapper.findAll('[data-testid="develop-variants-row"]');
    expect(rows).toHaveLength(2);
    expect(rows[0].text()).toContain("default");
    expect(rows[0].text()).toContain("3");
    expect(rows[1].text()).toContain("vc-abc123");
    // The primary variant exposes no delete control.
    expect(
      rows[0].find('[data-testid="develop-variants-delete"]').exists(),
    ).toBe(false);
    expect(
      rows[1].find('[data-testid="develop-variants-delete"]').exists(),
    ).toBe(true);
  });

  it("creates a virtual copy through the backend command and refreshes the list", async () => {
    const wrapper = mountPanel();
    await flushPromises();

    await wrapper.find('[data-testid="develop-variants-create"]').trigger("click");
    await flushPromises();

    expect(invokeMock).toHaveBeenCalledWith("develop_create_virtual_copy", {
      assetId: 42,
      fromVariantId: null,
    });
    // The list was refreshed after the creation (two list calls total).
    const listCalls = invokeMock.mock.calls.filter(
      ([command]) => command === "develop_list_variants",
    );
    expect(listCalls.length).toBeGreaterThanOrEqual(2);
  });

  it("resets a variant with its exact revision and refreshes", async () => {
    const wrapper = mountPanel();
    await flushPromises();

    const rows = wrapper.findAll('[data-testid="develop-variants-row"]');
    await rows[1].find('[data-testid="develop-variants-reset"]').trigger("click");
    await flushPromises();

    expect(invokeMock).toHaveBeenCalledWith("develop_reset_variant", {
      assetId: 42,
      variantId: "vc-abc123",
      expectedRevision: 1,
    });
  });

  it("deletes a virtual copy and refreshes", async () => {
    const wrapper = mountPanel();
    await flushPromises();

    const rows = wrapper.findAll('[data-testid="develop-variants-row"]');
    await rows[1].find('[data-testid="develop-variants-delete"]').trigger("click");
    await flushPromises();

    expect(invokeMock).toHaveBeenCalledWith("develop_delete_variant", {
      assetId: 42,
      variantId: "vc-abc123",
    });
  });

  it("surfaces backend failures as a visible error, never as success", async () => {
    invokeMock.mockImplementation((command: string) => {
      if (command === "develop_list_variants") return Promise.resolve([DEFAULT_VARIANT]);
      if (command === "develop_create_virtual_copy")
        return Promise.reject("virtual copy already exists at C:/lib/photo.arw.lapedit.v-vc-abc123.json");
      return Promise.resolve(undefined);
    });
    const wrapper = mountPanel();
    await flushPromises();

    await wrapper.find('[data-testid="develop-variants-create"]').trigger("click");
    await flushPromises();

    const error = wrapper.find('[data-testid="develop-variants-error"]');
    expect(error.exists()).toBe(true);
    expect(error.text()).toContain("virtual copy already exists");
  });

  it("does nothing when disabled or without a file", async () => {
    const wrapper = mount(VariantsPanel, {
      props: { fileId: null, disabled: false, active: true },
      global: { plugins: [makeI18n()] },
    });
    await flushPromises();
    expect(invokeMock).not.toHaveBeenCalled();

    const disabled = mount(VariantsPanel, {
      props: { fileId: 42, disabled: true, active: true },
      global: { plugins: [makeI18n()] },
    });
    await flushPromises();
    const create = disabled.find('[data-testid="develop-variants-create"]');
    expect(create.attributes("disabled")).toBeDefined();
  });
});
