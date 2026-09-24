<template>
  <aside class="w-full h-full rounded-box bg-base-200 flex flex-col overflow-hidden">
    <header class="my-2 px-2 flex items-center w-full shrink-0">
      <div class="flex-1 pl-1">
        <span class="text-sm font-semibold text-primary/70">{{ $t('agent.title') }}</span>
      </div>
      <div class="flex items-center gap-1">
        <TButton
          :icon="IconCopy"
          :buttonSize="'small'"
          :tooltip="$t('agent.copy')"
          :disabled="messages.length === 0"
          @click="copyChat"
        />
        <TButton
          :icon="IconDownload"
          :buttonSize="'small'"
          :tooltip="$t('agent.download')"
          :disabled="messages.length === 0"
          @click="downloadChat"
        />
        <button class="btn btn-xs btn-ghost" @click="$emit('close')">{{ $t('agent.close') }}</button>
      </div>
    </header>
    <div ref="scrollRef" class="flex-1 overflow-y-auto px-3 py-2 space-y-3">
      <p v-if="messages.length === 0" class="text-xs text-base-content/50 leading-5">{{ $t('agent.empty') }}</p>
      <article v-for="(message, index) in messages" :key="index" class="text-sm leading-5">
        <div class="text-[10px] uppercase tracking-widest text-base-content/30 mb-1">
          {{ message.role === 'user' ? $t('agent.you') : $t('agent.assistant') }}
        </div>
        <div class="agent-markdown" v-html="markdown(message.content)"></div>
        <div v-if="debugMode && message.trace?.length" class="mt-2 space-y-1 text-[11px] leading-4 text-base-content/35">
          <div v-for="(entry, entryIndex) in message.trace" :key="entryIndex" class="break-words">
            <template v-if="entry.kind === 'tool'">
              <span class="font-mono">{{ entry.tool }}</span>
              <span v-if="entry.arguments && Object.keys(entry.arguments).length" class="font-mono"> {{ formatTraceArgs(entry.arguments) }}</span>
              <div v-if="entry.summary">{{ entry.ok === false ? 'error: ' : '' }}{{ entry.summary }}</div>
            </template>
            <div v-else>{{ entry.text }}</div>
          </div>
        </div>
        <div v-if="message.files?.length" class="mt-2 grid grid-cols-4 gap-1">
          <button
            v-for="fileId in message.files"
            :key="fileId"
            type="button"
            class="block w-full cursor-pointer overflow-hidden rounded-box bg-base-300"
            :title="$t('agent.open_photo')"
            @click="openFile(fileId)"
          >
            <img
              :src="thumb(fileId)"
              class="w-full aspect-square object-cover"
              alt=""
            />
          </button>
        </div>
      </article>
      <div v-if="sending" class="flex items-center gap-2 py-1 text-xs text-base-content/60">
        <span class="loading loading-spinner loading-xs"></span>
        <span>{{ $t('agent.working') }}</span>
      </div>
      <p v-if="error" class="text-xs text-error">{{ error }}</p>
    </div>
    <form class="p-2 border-t border-base-content/10 flex gap-2" @submit.prevent="send">
      <textarea
        v-model="draft"
        rows="2"
        class="textarea textarea-sm flex-1"
        :placeholder="$t('agent.placeholder')"
        @keydown.enter.exact.prevent="send"
      />
      <button class="btn btn-sm btn-primary" :disabled="sending || !draft.trim()">
        {{ sending ? $t('agent.working') : $t('agent.send') }}
      </button>
    </form>
  </aside>
</template>

<script setup>
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { save } from '@tauri-apps/plugin-dialog';
import { writeTextFile } from '@tauri-apps/plugin-fs';
import { agentChat } from '@/common/api';
import { config } from '@/common/config';
import { IconCopy, IconDownload } from '@/common/icons';
import { renderMarkdown } from '@/common/markdown';
import { getThumbUrl } from '@/common/utils';
import TButton from '@/components/TButton.vue';

defineEmits(['close']);

const { t } = useI18n();

const debugMode = computed(() => Boolean(config.settings.debugMode));
const messages = ref([]);
const draft = ref('');
const sending = ref(false);
const error = ref('');
const scrollRef = ref(null);
const context = ref({ fileIds: [], focusedFileId: null });

function onContext(event) {
  const detail = event.detail || {};
  context.value = {
    fileIds: Array.isArray(detail.fileIds) ? detail.fileIds : [],
    focusedFileId: detail.focusedFileId || null,
  };
}

function thumb(fileId) {
  return getThumbUrl(fileId, false, 256);
}

function openFile(fileId) {
  window.dispatchEvent(new CustomEvent('lap-open-agent-file', { detail: { fileId: Number(fileId) } }));
}

function markdown(content) {
  return renderMarkdown(content);
}

function chatTranscript() {
  return messages.value.map((message) => {
    const who = message.role === 'user' ? t('agent.you') : t('agent.assistant');
    let body = `## ${who}\n\n${message.content || ''}`;
    if (debugMode.value && message.trace?.length) {
      const trace = message.trace.map((entry) => {
        if (entry.kind === 'tool') {
          const args = entry.arguments && Object.keys(entry.arguments).length ? ` ${JSON.stringify(entry.arguments)}` : '';
          return `- ${entry.tool}${args}${entry.summary ? `: ${entry.summary}` : ''}`;
        }
        return `- ${entry.text || ''}`;
      }).join('\n');
      body += `\n\n${trace}`;
    }
    return body;
  }).join('\n\n');
}

async function copyChat() {
  if (messages.value.length === 0) return;
  try {
    await navigator.clipboard.writeText(chatTranscript());
  } catch (err) {
    error.value = String(err?.message || err);
  }
}

async function downloadChat() {
  if (messages.value.length === 0) return;
  const stamp = new Date().toISOString().slice(0, 16).replace(/[:T]/g, '-');
  try {
    const destPath = await save({
      title: t('agent.download'),
      defaultPath: `lap-chat-${stamp}.md`,
      filters: [{ name: 'Markdown', extensions: ['md'] }],
    });
    if (!destPath) return;
    await writeTextFile(destPath, chatTranscript());
  } catch (err) {
    error.value = String(err?.message || err);
  }
}

function formatTraceArgs(args) {
  const text = JSON.stringify(args);
  return text.length > 240 ? `${text.slice(0, 240)}…` : text;
}

async function scrollToEnd() {
  await nextTick();
  if (scrollRef.value) scrollRef.value.scrollTop = scrollRef.value.scrollHeight;
}

async function send() {
  const text = draft.value.trim();
  if (!text || sending.value) return;
  messages.value.push({ role: 'user', content: text });
  draft.value = '';
  sending.value = true;
  error.value = '';
  await scrollToEnd();
  try {
    const fileIds = context.value.fileIds?.length
      ? context.value.fileIds
      : (context.value.focusedFileId ? [context.value.focusedFileId] : []);
    const response = await agentChat(
      messages.value.map((message) => ({ role: message.role, content: message.content })),
      { fileIds, focusedFileId: context.value.focusedFileId },
    );
    const files = (response.actions || [])
      .filter((action) => action.type === 'show_files')
      .flatMap((action) => action.fileIds || [])
      .slice(0, 12);
    messages.value.push({
      role: 'assistant',
      content: response.message || '',
      files,
      trace: response.trace || [],
    });
  } catch (err) {
    error.value = String(err?.message || err);
  } finally {
    sending.value = false;
    await scrollToEnd();
  }
}

onMounted(() => window.addEventListener('lap-agent-context', onContext));
onBeforeUnmount(() => window.removeEventListener('lap-agent-context', onContext));
</script>

<style scoped>
.agent-markdown :deep(p) {
  margin: 0.35rem 0;
}
.agent-markdown :deep(h1),
.agent-markdown :deep(h2),
.agent-markdown :deep(h3) {
  margin: 0.6rem 0 0.25rem;
  font-weight: 650;
  line-height: 1.3;
}
.agent-markdown :deep(h1) { font-size: 1.05rem; }
.agent-markdown :deep(h2) { font-size: 0.95rem; }
.agent-markdown :deep(h3) { font-size: 0.85rem; }
.agent-markdown :deep(ul),
.agent-markdown :deep(ol) {
  margin: 0.35rem 0;
  padding-left: 1.15rem;
}
.agent-markdown :deep(ul) { list-style: disc; }
.agent-markdown :deep(ol) { list-style: decimal; }
.agent-markdown :deep(li) { margin: 0.1rem 0; }
.agent-markdown :deep(a) {
  color: var(--color-primary, #3b82f6);
  text-decoration: underline;
}
.agent-markdown :deep(code) {
  padding: 0.05rem 0.25rem;
  border-radius: 0.25rem;
  background: color-mix(in oklab, var(--color-base-content) 8%, transparent);
  font-size: 0.85em;
}
.agent-markdown :deep(pre) {
  margin: 0.4rem 0;
  padding: 0.5rem 0.6rem;
  overflow-x: auto;
  border-radius: 0.5rem;
  background: color-mix(in oklab, var(--color-base-content) 8%, transparent);
}
.agent-markdown :deep(pre code) {
  padding: 0;
  background: transparent;
}
.agent-markdown :deep(.agent-table-wrap) {
  margin: 0.45rem 0;
  overflow-x: auto;
}
.agent-markdown :deep(table) {
  width: 100%;
  border-collapse: collapse;
  font-size: 0.78rem;
  line-height: 1.35;
}
.agent-markdown :deep(th),
.agent-markdown :deep(td) {
  padding: 0.28rem 0.4rem;
  border: 1px solid color-mix(in oklab, var(--color-base-content) 15%, transparent);
  vertical-align: top;
}
.agent-markdown :deep(th) {
  font-weight: 650;
  background: color-mix(in oklab, var(--color-base-content) 6%, transparent);
}
.agent-markdown :deep(blockquote) {
  margin: 0.4rem 0;
  padding-left: 0.6rem;
  border-left: 2px solid color-mix(in oklab, var(--color-base-content) 20%, transparent);
  color: color-mix(in oklab, var(--color-base-content) 70%, transparent);
}
</style>
