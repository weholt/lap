<template>
  <div class="flex flex-col space-y-3 text-sm">
    <section class="rounded-box p-3 space-y-3 bg-base-300/30 border border-base-content/5">
      <div class="font-bold uppercase text-[10px] tracking-widest text-base-content/30">{{ $t('settings.agent.provider') }}</div>
      <label class="flex flex-col gap-1">
        <span class="text-xs text-base-content/50">{{ $t('settings.agent.provider') }}</span>
        <select v-model="provider" class="select select-sm" @change="applyPreset">
          <option value="openai">OpenAI</option>
          <option value="openrouter">OpenRouter</option>
          <option value="ollama">Ollama</option>
          <option value="custom">{{ $t('settings.agent.custom') }}</option>
        </select>
      </label>
      <label class="flex flex-col gap-1">
        <span class="text-xs text-base-content/50">{{ $t('settings.agent.base_url') }}</span>
        <input v-model="baseUrl" type="text" class="input input-sm" spellcheck="false" />
      </label>
      <label class="flex flex-col gap-1">
        <span class="text-xs text-base-content/50">{{ $t('settings.agent.model') }}</span>
        <div class="flex gap-2">
          <input v-model="model" type="text" class="input input-sm flex-1" list="agent-models" spellcheck="false" />
          <button class="btn btn-sm" :disabled="loadingModels" @click="loadModels">{{ $t('settings.agent.load_models') }}</button>
        </div>
        <span class="text-[11px] text-base-content/40">{{ $t('settings.agent.model_hint') }}</span>
        <datalist id="agent-models">
          <option v-for="item in models" :key="item" :value="item" />
        </datalist>
      </label>
      <label class="flex flex-col gap-1">
        <span class="text-xs text-base-content/50">{{ $t('settings.agent.api_key') }}</span>
        <input v-model="apiKey" type="password" class="input input-sm" :placeholder="apiKeyHint" spellcheck="false" autocomplete="off" />
        <span class="text-[11px] text-base-content/40">{{ $t('settings.agent.api_key_hint') }}</span>
      </label>
      <button class="btn btn-sm btn-primary" :disabled="saving" @click="save">{{ $t('settings.agent.save') }}</button>
      <p v-if="status" class="text-xs text-success">{{ status }}</p>
      <p v-if="error" class="text-xs text-error">{{ error }}</p>
    </section>

    <section class="rounded-box p-3 space-y-3 bg-base-300/30 border border-base-content/5">
      <div class="flex items-center justify-between">
        <div class="font-bold uppercase text-[10px] tracking-widest text-base-content/30">{{ $t('settings.agent.mcp') }}</div>
        <input v-model="mcpEnabled" type="checkbox" class="toggle toggle-primary toggle-sm" />
      </div>
      <p class="text-[11px] text-base-content/50">{{ $t('settings.agent.mcp_hint') }}</p>
      <label class="flex flex-col gap-1">
        <span class="text-xs text-base-content/50">{{ $t('settings.agent.mcp_port') }}</span>
        <input v-model.number="mcpPort" type="number" min="1024" max="65535" class="input input-sm w-32" />
      </label>
      <div class="text-xs break-all">
        <div class="text-base-content/40">{{ $t('settings.agent.mcp_url') }}</div>
        <div>{{ mcpUrl }}</div>
      </div>
      <div class="text-xs break-all">
        <div class="text-base-content/40">{{ $t('settings.agent.mcp_token') }}</div>
        <div class="font-mono">{{ mcpToken }}</div>
      </div>
      <div class="flex gap-2">
        <button class="btn btn-sm" @click="copyConfig">{{ $t('settings.agent.copy_config') }}</button>
        <button class="btn btn-sm" @click="regenerate">{{ $t('settings.agent.new_token') }}</button>
      </div>
      <p class="text-[11px]" :class="mcpRunning ? 'text-success' : 'text-warning'">
        {{ mcpRunning ? $t('settings.agent.mcp_running') : $t('settings.agent.mcp_stopped') }}
        <span v-if="mcpError"> — {{ mcpError }}</span>
      </p>
      <pre class="text-[11px] whitespace-pre-wrap bg-base-100/40 rounded-box p-2">{{ configSnippet }}</pre>
    </section>
  </div>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { getAgentSettings, listAgentModels, saveAgentSettings } from '@/common/api';
import { cachedModelsFor, storeCachedModels } from '@/common/agentModelCache';

const { t } = useI18n();
const provider = ref('openai');
const baseUrl = ref('');
const model = ref('');
const apiKey = ref('');
const apiKeyHint = ref('');
const mcpEnabled = ref(true);
const mcpPort = ref(47321);
const mcpToken = ref('');
const mcpUrl = ref('');
const mcpRunning = ref(false);
const mcpError = ref('');
const models = ref([]);
const saving = ref(false);
const loadingModels = ref(false);
const status = ref('');
const error = ref('');

const presets = {
  openai: 'https://api.openai.com/v1',
  openrouter: 'https://openrouter.ai/api/v1',
  ollama: 'http://127.0.0.1:11434/v1',
};

const configSnippet = computed(() => `{
  "mcpServers": {
    "lap": {
      "url": "${mcpUrl.value}",
      "headers": { "Authorization": "Bearer ${mcpToken.value}" }
    }
  }
}`);

function restoreCachedModels() {
  models.value = cachedModelsFor(provider.value);
}

function applyPreset() {
  if (presets[provider.value]) baseUrl.value = presets[provider.value];
  restoreCachedModels();
}

function applyView(view) {
  provider.value = view.provider || 'openai';
  baseUrl.value = view.baseUrl || '';
  model.value = view.model || '';
  apiKeyHint.value = view.apiKeyHint || '';
  mcpEnabled.value = view.mcpEnabled !== false;
  mcpPort.value = Number(view.mcpPort || 47321);
  mcpToken.value = view.mcpToken || '';
  mcpUrl.value = view.mcpUrl || '';
  mcpRunning.value = Boolean(view.mcpRunning);
  mcpError.value = view.mcpError || '';
}

async function save(regenerateToken = false) {
  saving.value = true;
  status.value = '';
  error.value = '';
  try {
    const view = await saveAgentSettings({
      provider: provider.value,
      baseUrl: baseUrl.value,
      model: model.value,
      apiKey: apiKey.value.length > 0 ? apiKey.value : null,
      mcpEnabled: mcpEnabled.value,
      mcpPort: Number(mcpPort.value || 47321),
      regenerateToken: regenerateToken === true,
    });
    apiKey.value = '';
    applyView(view);
    status.value = t('settings.agent.saved');
  } catch (err) {
    error.value = String(err);
  } finally {
    saving.value = false;
  }
}

async function regenerate() {
  await save(true);
}

async function loadModels() {
  const cached = cachedModelsFor(provider.value);
  if (cached.length > 0) {
    models.value = cached;
    return;
  }
  loadingModels.value = true;
  error.value = '';
  try {
    if (apiKey.value || baseUrl.value) await save(false);
    models.value = await listAgentModels();
    storeCachedModels(provider.value, models.value);
  } catch (err) {
    error.value = String(err);
  } finally {
    loadingModels.value = false;
  }
}

async function copyConfig() {
  try {
    await navigator.clipboard.writeText(configSnippet.value);
    status.value = t('settings.agent.copied');
  } catch (err) {
    error.value = String(err);
  }
}

onMounted(async () => {
  try {
    applyView(await getAgentSettings());
    restoreCachedModels();
  } catch (err) {
    error.value = String(err);
  }
});
</script>
