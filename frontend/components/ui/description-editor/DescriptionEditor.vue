<script setup lang="ts">
import { useEditor, EditorContent } from '@tiptap/vue-3'
import StarterKit from '@tiptap/starter-kit'
import Link from '@tiptap/extension-link'
import Placeholder from '@tiptap/extension-placeholder'

const props = withDefaults(defineProps<{
  /** 'html' for eBay/Squarespace/Amazon, 'text' for XMR Bazaar */
  outputFormat?: 'html' | 'text'
  disabled?: boolean
}>(), {
  outputFormat: 'html',
  disabled: false,
})

const model = defineModel<string>({ default: '' })

const editing = ref(false)

// ── Editor instance ──
const editor = useEditor({
  extensions: [
    StarterKit.configure({
      heading: { levels: [2, 3, 4] },
    }),
    Link.configure({
      openOnClick: false,
      HTMLAttributes: { class: 'editor-link' },
    }),
    Placeholder.configure({
      placeholder: 'Write a description...',
    }),
  ],
  content: model.value || '',
  editable: true,
  onUpdate({ editor: ed }) {
    model.value = ed.getHTML()
  },
})

// Sync external changes into editor when switching to edit mode
watch(editing, (isEditing) => {
  if (isEditing && editor.value) {
    const current = editor.value.getHTML()
    if (current !== model.value) {
      editor.value.commands.setContent(model.value || '')
    }
  }
})

function enterEdit() {
  if (props.disabled) return
  editing.value = true
  nextTick(() => editor.value?.commands.focus('end'))
}

function exitEdit() {
  editing.value = false
}

// ── Preview helpers ──
function stripHtml(html: string): string {
  if (!html) return ''
  const doc = new DOMParser().parseFromString(html, 'text/html')
  const blockTags = new Set([
    'P', 'DIV', 'BR', 'HR', 'H1', 'H2', 'H3', 'H4', 'H5', 'H6',
    'UL', 'OL', 'BLOCKQUOTE', 'SECTION', 'ARTICLE', 'TR', 'HEADER', 'FOOTER',
  ])
  function walk(node: Node): string {
    if (node.nodeType === Node.TEXT_NODE) return node.textContent || ''
    if (node.nodeType !== Node.ELEMENT_NODE) return ''
    const tag = (node as Element).tagName
    const isBlock = blockTags.has(tag)
    const isLi = tag === 'LI'
    let result = ''
    if (isBlock || isLi) result += '\n'
    if (isLi) result += '\u00b7 '
    for (const child of Array.from(node.childNodes)) {
      result += walk(child)
    }
    if (isBlock && !result.endsWith('\n')) result += '\n'
    return result
  }
  return walk(doc.body).replace(/\n{3,}/g, '\n\n').trim()
}

const previewText = computed(() => stripHtml(model.value))

const isEmpty = computed(() => {
  if (!model.value) return true
  const stripped = model.value.replace(/<[^>]*>/g, '').trim()
  return stripped.length === 0
})

// ── Toolbar actions ──
function toggleBold() { editor.value?.chain().focus().toggleBold().run() }
function toggleItalic() { editor.value?.chain().focus().toggleItalic().run() }
function toggleStrike() { editor.value?.chain().focus().toggleStrike().run() }
function toggleHeading(level: 2 | 3 | 4) { editor.value?.chain().focus().toggleHeading({ level }).run() }
function toggleBulletList() { editor.value?.chain().focus().toggleBulletList().run() }
function toggleOrderedList() { editor.value?.chain().focus().toggleOrderedList().run() }
function toggleBlockquote() { editor.value?.chain().focus().toggleBlockquote().run() }
function toggleCode() { editor.value?.chain().focus().toggleCode().run() }
function setHorizontalRule() { editor.value?.chain().focus().setHorizontalRule().run() }
function undo() { editor.value?.chain().focus().undo().run() }
function redo() { editor.value?.chain().focus().redo().run() }

function setLink() {
  const prev = editor.value?.getAttributes('link').href || ''
  const url = window.prompt('URL', prev)
  if (url === null) return
  if (url === '') {
    editor.value?.chain().focus().extendMarkRange('link').unsetLink().run()
  } else {
    editor.value?.chain().focus().extendMarkRange('link').setLink({ href: url }).run()
  }
}

function isActive(name: string, attrs?: Record<string, any>) {
  return editor.value?.isActive(name, attrs) ?? false
}
</script>

<template>
  <div class="desc-editor">
    <!-- ═══ Preview mode ═══ -->
    <Splatter
      v-if="!editing"
      frozen
      class="desc-editor__preview-wrap"
      :colors="['139,92,246', '71,85,105', '100,116,139']"
      :opacity-ranges="[[0.15, 0.35], [0.15, 0.35], [0.05, 0.3]]"
      :sizes="['80%', '75%', '65%']"
      :edge-opacity="0.005"
    >
      <div v-if="isEmpty" class="desc-editor__empty">
        No description
      </div>
      <template v-else>
        <!-- Plain text preview for XMR Bazaar -->
        <pre v-if="outputFormat === 'text'" class="desc-editor__preview-text">{{ previewText }}</pre>
        <!-- Rendered HTML preview for other platforms -->
        <div v-else class="desc-editor__preview-html" v-html="model" />
      </template>

      <Button
        v-if="!disabled"
        class="w-full relative z-[1] mt-2"
        size="sm"
        @click="enterEdit"
      >
        Edit description
      </Button>
    </Splatter>

    <!-- ═══ Edit mode ═══ -->
    <div v-else class="desc-editor__edit-wrap">
      <!-- Toolbar (ground glass) -->
      <GroundGlass class="desc-editor__toolbar-glass">
        <div class="desc-editor__toolbar">
          <div class="desc-editor__toolbar-group">
            <button :class="{ active: isActive('bold') }" title="Bold" @click="toggleBold">
              <svg class="w-3.5 h-3.5" viewBox="0 0 15 15" fill="none"><path d="M5.105 12V3h3.413c.836 0 1.49.197 1.964.591.478.394.717.92.717 1.577 0 .47-.124.862-.371 1.177-.244.311-.58.524-1.008.637v.064c.549.08.98.3 1.295.66.318.356.477.8.477 1.33 0 .733-.263 1.31-.789 1.732-.522.419-1.221.628-2.098.628H5.105ZM6.5 6.753h1.86c.528 0 .94-.115 1.236-.345.298-.234.447-.563.447-.987 0-.389-.14-.695-.42-.918-.277-.223-.67-.334-1.179-.334H6.5v2.584Zm0 1.1v2.962h2.03c.545 0 .97-.126 1.275-.378.309-.256.463-.605.463-1.048 0-.447-.158-.798-.475-1.054-.314-.256-.747-.384-1.299-.384H6.5Z" fill="currentColor" /></svg>
            </button>
            <button :class="{ active: isActive('italic') }" title="Italic" @click="toggleItalic">
              <svg class="w-3.5 h-3.5" viewBox="0 0 15 15" fill="none"><path d="M5.675 12l2.13-9h1.508l-2.13 9H5.675Z" fill="currentColor" /></svg>
            </button>
            <button :class="{ active: isActive('strike') }" title="Strikethrough" @click="toggleStrike">
              <svg class="w-3.5 h-3.5" viewBox="0 0 15 15" fill="none"><path d="M5.833 6.5H2v-1h11v1H9.167c.339.328.583.733.583 1.25 0 1.242-1.296 2.25-2.75 2.25S4.25 8.992 4.25 7.75c0-.517.244-.922.583-1.25ZM7 5c-1.035 0-1.75.56-1.75 1.125h-1.5C3.75 4.717 5.2 3.5 7 3.5s3.25 1.217 3.25 2.625h-1.5C8.75 5.56 8.035 5 7 5Z" fill="currentColor" /></svg>
            </button>
            <button :class="{ active: isActive('code') }" title="Code" @click="toggleCode">
              <svg class="w-3.5 h-3.5" viewBox="0 0 15 15" fill="none"><path d="M9.964 2.686l.07.995-1 .07-.07-.995 1-.07Zm-1.11 7.636l-.07-.995 1-.07.07.995-1 .07ZM5.5 4.964l-3 2.5.642.77L6.142 5.73 5.5 4.964ZM3.142 8.27l3 2.5.642-.77L3.784 7.5l-.642.77ZM9.5 10.036l3-2.5-.642-.77-3 2.504.642.766ZM11.858 6.73l-3-2.5-.642.77L11.216 7.5l.642-.77Z" fill="currentColor" /></svg>
            </button>
          </div>

          <span class="desc-editor__toolbar-sep" />

          <div class="desc-editor__toolbar-group">
            <button :class="{ active: isActive('heading', { level: 2 }) }" title="Heading 2" @click="toggleHeading(2)">H2</button>
            <button :class="{ active: isActive('heading', { level: 3 }) }" title="Heading 3" @click="toggleHeading(3)">H3</button>
            <button :class="{ active: isActive('heading', { level: 4 }) }" title="Heading 4" @click="toggleHeading(4)">H4</button>
          </div>

          <span class="desc-editor__toolbar-sep" />

          <div class="desc-editor__toolbar-group">
            <button :class="{ active: isActive('bulletList') }" title="Bullet list" @click="toggleBulletList">
              <svg class="w-3.5 h-3.5" viewBox="0 0 15 15" fill="none"><path d="M1.5 5.25a1.125 1.125 0 100-2.25 1.125 1.125 0 000 2.25ZM4 4h10v1H4V4Zm0 3.5h10v1H4v-1Zm0 3.5h10v1H4v-1ZM1.5 8.75a1.125 1.125 0 100-2.25 1.125 1.125 0 000 2.25Zm0 3.5a1.125 1.125 0 100-2.25 1.125 1.125 0 000 2.25Z" fill="currentColor" /></svg>
            </button>
            <button :class="{ active: isActive('orderedList') }" title="Ordered list" @click="toggleOrderedList">
              <svg class="w-3.5 h-3.5" viewBox="0 0 15 15" fill="none"><path d="M2 3.5V1h-.5v.5H1v1h.5V3h-.25v.5h1.5V3H2Zm2 .5h10v1H4V4Zm0 3.5h10v1H4v-1Zm0 3.5h10v1H4v-1ZM1.1 7v.5h1.25l-1.25 1.5v.5h2V9h-1.25L3.1 7.5V7h-2Zm-.1 4v.5h.9l-.6.6v.4h.6V13h.5v-.5h.6v-.9L2.2 12.6V11.5h.9V11H1Z" fill="currentColor" /></svg>
            </button>
            <button :class="{ active: isActive('blockquote') }" title="Blockquote" @click="toggleBlockquote">
              <svg class="w-3.5 h-3.5" viewBox="0 0 15 15" fill="none"><path d="M9.425 4.1a3.5 3.5 0 00-3.2 3.4H7.5a1.9 1.9 0 110 3.8 1.9 1.9 0 01-1.874-2.2L5.6 8.85a3.78 3.78 0 013.825-4.75Zm-5 0a3.5 3.5 0 00-3.2 3.4H2.5a1.9 1.9 0 110 3.8 1.9 1.9 0 01-1.874-2.2L.6 8.85A3.78 3.78 0 014.425 4.1Z" fill="currentColor" /></svg>
            </button>
            <button title="Horizontal rule" @click="setHorizontalRule">
              <svg class="w-3.5 h-3.5" viewBox="0 0 15 15" fill="none"><path d="M2 7.5h11v1H2z" fill="currentColor" /></svg>
            </button>
          </div>

          <span class="desc-editor__toolbar-sep" />

          <div class="desc-editor__toolbar-group">
            <button :class="{ active: isActive('link') }" title="Link" @click="setLink">
              <svg class="w-3.5 h-3.5" viewBox="0 0 15 15" fill="none"><path d="M4.625 4.068C5.558 3.136 7.058 3.136 7.99 4.068l.707.707-.707.707-.707-.707a1.414 1.414 0 00-2 0l-2.121 2.121a1.414 1.414 0 000 2l.707.707-.707.707-.707-.707a2.414 2.414 0 010-3.414l2.17-2.121ZM10.375 10.932c-.933.932-2.433.932-3.365 0l-.707-.707.707-.707.707.707a1.414 1.414 0 002 0l2.121-2.121a1.414 1.414 0 000-2l-.707-.707.707-.707.707.707a2.414 2.414 0 010 3.414l-2.17 2.121ZM6.182 9.525l3.536-3.536.707.707-3.536 3.536-.707-.707Z" fill="currentColor" /></svg>
            </button>
          </div>

          <!-- Spacer -->
          <div class="flex-1" />

          <div class="desc-editor__toolbar-group">
            <button title="Undo" @click="undo">
              <svg class="w-3.5 h-3.5" viewBox="0 0 15 15" fill="none"><path d="M2.146 6.854a.5.5 0 010-.708l3-3 .708.708L3.707 6H9.5a3.5 3.5 0 110 7H7v-1h2.5a2.5 2.5 0 000-5H3.707l2.147 2.146-.708.708-3-3Z" fill="currentColor" /></svg>
            </button>
            <button title="Redo" @click="redo">
              <svg class="w-3.5 h-3.5" viewBox="0 0 15 15" fill="none"><path d="M12.854 6.854a.5.5 0 000-.708l-3-3-.708.708L11.293 6H5.5a3.5 3.5 0 100 7H8v-1H5.5a2.5 2.5 0 010-5h5.793L9.146 9.146l.708.708 3-3Z" fill="currentColor" /></svg>
            </button>
          </div>

          <span class="desc-editor__toolbar-sep" />

          <button class="desc-editor__done-btn" @click="exitEdit">Done</button>
        </div>
      </GroundGlass>

      <!-- Editor content area (splatter background) -->
      <Splatter
        class="desc-editor__content-wrap"
        :colors="['139,92,246', '71,85,105', '100,116,139']"
        :opacity-ranges="[[0.15, 0.35], [0.15, 0.35], [0.05, 0.3]]"
        :sizes="['80%', '75%', '65%']"
        :edge-opacity="0.005"
      >
        <!-- Format hint -->
        <div v-if="outputFormat === 'text'" class="desc-editor__format-hint">
          <svg class="w-3 h-3 shrink-0" viewBox="0 0 15 15" fill="none"><path d="M7.5.877a6.623 6.623 0 100 13.246A6.623 6.623 0 007.5.877ZM1.827 7.5a5.673 5.673 0 1111.346 0 5.673 5.673 0 01-11.346 0ZM7.5 4a.5.5 0 00-.5.5v4a.5.5 0 001 0v-4a.5.5 0 00-.5-.5Zm0 7.25a.625.625 0 100-1.25.625.625 0 000 1.25Z" fill="currentColor" fill-rule="evenodd" clip-rule="evenodd" /></svg>
          This platform only supports plain text. Formatting will be stripped on publish.
        </div>

        <EditorContent :editor="editor" class="desc-editor__content" />
      </Splatter>
    </div>
  </div>
</template>

<style scoped>
/* ═══════════════════════════════════════════
   Preview mode
   ═══════════════════════════════════════════ */
.desc-editor__preview-wrap {
}

.desc-editor__empty {
  position: relative;
  z-index: 1;
  padding: 12px 12px;
  font-size: 13px;
  color: rgba(148, 163, 184, 0.3);
  font-style: italic;
}

/* Rendered HTML preview */
.desc-editor__preview-html {
  position: relative;
  z-index: 1;
  font-size: 13px;
  line-height: 1.7;
  color: rgba(226, 232, 240, 0.8);
  max-height: 240px;
  overflow-y: auto;
  padding: 8px 12px;
}

.desc-editor__preview-html :deep(h1),
.desc-editor__preview-html :deep(h2),
.desc-editor__preview-html :deep(h3),
.desc-editor__preview-html :deep(h4) {
  font-weight: 600;
  color: rgba(226, 232, 240, 0.95);
  margin: 0.8em 0 0.4em;
}
.desc-editor__preview-html :deep(h2) { font-size: 16px; }
.desc-editor__preview-html :deep(h3) { font-size: 14px; }
.desc-editor__preview-html :deep(h4) { font-size: 13px; }
.desc-editor__preview-html :deep(p) { margin: 0.5em 0; }
.desc-editor__preview-html :deep(ul),
.desc-editor__preview-html :deep(ol) {
  padding-left: 1.5em;
  margin: 0.5em 0;
}
.desc-editor__preview-html :deep(li) { margin: 0.2em 0; }
.desc-editor__preview-html :deep(a) {
  color: rgba(139, 92, 246, 0.8);
  text-decoration: underline;
  text-underline-offset: 2px;
}
.desc-editor__preview-html :deep(blockquote) {
  border-left: 2px solid rgba(139, 92, 246, 0.3);
  padding-left: 12px;
  color: rgba(196, 205, 214, 0.6);
  margin: 0.5em 0;
}
.desc-editor__preview-html :deep(code) {
  font-family: ui-monospace, SFMono-Regular, 'SF Mono', Menlo, monospace;
  font-size: 0.9em;
  background: rgba(71, 85, 105, 0.2);
  padding: 1px 5px;
  border-radius: 3px;
}
.desc-editor__preview-html :deep(hr) {
  border: none;
  border-top: 1px solid rgba(71, 85, 105, 0.25);
  margin: 1em 0;
}
.desc-editor__preview-html :deep(strong) { font-weight: 600; }

/* Plain text preview */
.desc-editor__preview-text {
  position: relative;
  z-index: 1;
  font-family: ui-monospace, SFMono-Regular, 'SF Mono', Menlo, monospace;
  font-size: 12px;
  line-height: 1.7;
  color: rgba(226, 232, 240, 0.65);
  max-height: 240px;
  overflow-y: auto;
  padding: 8px 12px;
  white-space: pre-wrap;
  word-break: break-word;
  margin: 0;
}


/* ═══════════════════════════════════════════
   Editor mode — toolbar & content
   ═══════════════════════════════════════════ */

.desc-editor__toolbar-glass {
  border-radius: 0.5rem 0.5rem 0 0;
}

/* ── Toolbar ── */
.desc-editor__toolbar {
  position: relative;
  z-index: 1;
  display: flex;
  align-items: center;
  gap: 2px;
  padding: 4px 6px;
  flex-wrap: wrap;
}

.desc-editor__toolbar-group {
  display: inline-flex;
  align-items: center;
  gap: 1px;
}

.desc-editor__toolbar button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 26px;
  height: 26px;
  padding: 0 5px;
  font-size: 11px;
  font-weight: 600;
  color: rgba(226, 232, 240, 0.7);
  background: transparent;
  border: none;
  border-radius: 0.25rem;
  cursor: pointer;
  outline: none;
  transition: color 0.15s ease, background 0.15s ease;
}
.desc-editor__toolbar button:hover {
  color: rgba(226, 232, 240, 0.95);
  background: rgba(139, 92, 246, 0.12);
}
.desc-editor__toolbar button.active {
  color: rgba(139, 92, 246, 1);
  background: rgba(139, 92, 246, 0.15);
}

.desc-editor__toolbar-sep {
  width: 1px;
  height: 16px;
  margin: 0 4px;
  background: rgba(71, 85, 105, 0.25);
}

.desc-editor__done-btn {
  font-size: 11px !important;
  font-weight: 500 !important;
  color: rgba(139, 92, 246, 0.7) !important;
  padding: 0 8px !important;
}
.desc-editor__done-btn:hover {
  color: rgba(139, 92, 246, 1) !important;
}

/* ── Content wrapper (splatter) ── */
.desc-editor__content-wrap {
  border-radius: 0;
}

/* ── Format hint ── */
.desc-editor__format-hint {
  position: relative;
  z-index: 1;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 10px;
  font-size: 11px;
  color: rgba(251, 191, 36, 0.6);
  background: rgba(251, 191, 36, 0.04);
  border-bottom: 1px solid rgba(71, 85, 105, 0.15);
}

/* ── Editor content area ── */
.desc-editor__content {
  position: relative;
  z-index: 1;
  min-height: 320px;
  max-height: 800px;
  overflow-y: auto;
  padding: 12px 14px;
}

.desc-editor__content :deep(.tiptap) {
  outline: none;
  font-size: 13px;
  line-height: 1.7;
  color: rgba(226, 232, 240, 0.85);
}

.desc-editor__content :deep(.tiptap p.is-editor-empty:first-child::before) {
  content: attr(data-placeholder);
  float: left;
  color: rgba(148, 163, 184, 0.25);
  pointer-events: none;
  height: 0;
}

.desc-editor__content :deep(.tiptap h2) { font-size: 16px; font-weight: 600; margin: 0.8em 0 0.4em; color: rgba(226, 232, 240, 0.95); }
.desc-editor__content :deep(.tiptap h3) { font-size: 14px; font-weight: 600; margin: 0.8em 0 0.4em; color: rgba(226, 232, 240, 0.95); }
.desc-editor__content :deep(.tiptap h4) { font-size: 13px; font-weight: 600; margin: 0.8em 0 0.4em; color: rgba(226, 232, 240, 0.95); }
.desc-editor__content :deep(.tiptap p) { margin: 0.4em 0; }
.desc-editor__content :deep(.tiptap ul),
.desc-editor__content :deep(.tiptap ol) { padding-left: 1.5em; margin: 0.4em 0; }
.desc-editor__content :deep(.tiptap li) { margin: 0.15em 0; }
.desc-editor__content :deep(.tiptap blockquote) {
  border-left: 2px solid rgba(139, 92, 246, 0.3);
  padding-left: 12px;
  color: rgba(196, 205, 214, 0.6);
  margin: 0.5em 0;
}
.desc-editor__content :deep(.tiptap a),
.desc-editor__content :deep(.tiptap .editor-link) {
  color: rgba(139, 92, 246, 0.8);
  text-decoration: underline;
  text-underline-offset: 2px;
  cursor: pointer;
}
.desc-editor__content :deep(.tiptap code) {
  font-family: ui-monospace, SFMono-Regular, 'SF Mono', Menlo, monospace;
  font-size: 0.9em;
  background: rgba(71, 85, 105, 0.25);
  padding: 1px 5px;
  border-radius: 3px;
}
.desc-editor__content :deep(.tiptap hr) {
  border: none;
  border-top: 1px solid rgba(71, 85, 105, 0.25);
  margin: 1em 0;
}
.desc-editor__content :deep(.tiptap strong) { font-weight: 600; }
</style>
