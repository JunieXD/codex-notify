<script setup lang="ts">
import { useData } from 'vitepress'
import { nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'

let nextDiagramId = 0

const props = withDefaults(
  defineProps<{
    source: string
    label?: string
  }>(),
  {
    label: 'codex-notify 工作流程图'
  }
)

const container = ref<HTMLElement | null>(null)
const { isDark } = useData()
let disposed = false
let renderVersion = 0

async function renderDiagram() {
  const target = container.value
  if (!target || disposed) return

  const currentVersion = ++renderVersion
  target.classList.remove('has-error')
  target.textContent = '正在生成图表…'

  try {
    const { default: mermaid } = await import('mermaid')
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: 'strict',
      theme: isDark.value ? 'dark' : 'default',
      fontFamily: 'inherit'
    })
    const id = `mermaid-${Date.now()}-${++nextDiagramId}`
    const { svg, bindFunctions } = await mermaid.render(id, decodeURIComponent(props.source))

    if (disposed || currentVersion !== renderVersion || !container.value) return
    container.value.innerHTML = svg
    bindFunctions?.(container.value)
  } catch (error) {
    if (disposed || currentVersion !== renderVersion || !container.value) return
    container.value.classList.add('has-error')
    container.value.textContent = '图表暂时无法显示，请刷新页面重试。'
    console.error('无法渲染 Mermaid 图表', error)
  }
}

onMounted(() => {
  void renderDiagram()
})

watch([() => props.source, isDark], async () => {
  await nextTick()
  void renderDiagram()
})

onBeforeUnmount(() => {
  disposed = true
  renderVersion += 1
})
</script>

<template>
  <figure class="mermaid-diagram">
    <div
      ref="container"
      class="mermaid-diagram__canvas"
      role="img"
      :aria-label="label"
    >
      正在生成图表…
    </div>
  </figure>
</template>
