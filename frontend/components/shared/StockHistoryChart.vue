<script setup lang="ts">
import { Line } from 'vue-chartjs'
import {
  Chart as ChartJS,
  CategoryScale,
  LinearScale,
  PointElement,
  LineElement,
  Tooltip,
  Filler,
  type ChartData,
  type ChartOptions,
} from 'chart.js'

// N-11.8: chart.js/vue-chartjs are installed deps; this is the first
// component that actually imports them.
ChartJS.register(CategoryScale, LinearScale, PointElement, LineElement, Tooltip, Filler)

const props = defineProps<{
  history: [string, number][]
  lowStockThreshold?: number | null
}>()

function formatLabel(ts: string): string {
  // recorded_at is SQLite "YYYY-MM-DD HH:MM:SS". Keep it compact so a
  // sparkline in a table row stays readable.
  const [date, time] = ts.split(' ')
  if (!date) return ts
  const parts = date.split('-')
  if (parts.length !== 3) return ts
  const hm = (time ?? '').slice(0, 5)
  return hm ? `${parts[1]}/${parts[2]} ${hm}` : `${parts[1]}/${parts[2]}`
}

const chartData = computed<ChartData<'line'>>(() => {
  const datasets: ChartData<'line'>['datasets'] = [
    {
      label: 'Stock',
      data: props.history.map(([, qty]) => qty),
      borderColor: '#8b5cf6',
      backgroundColor: 'rgba(139, 92, 246, 0.16)',
      fill: true,
      tension: 0.35,
      pointRadius: 0,
      pointHoverRadius: 4,
      pointHoverBackgroundColor: '#8b5cf6',
      borderWidth: 1.5,
    },
  ]
  if (props.lowStockThreshold != null && props.history.length > 0) {
    datasets.push({
      label: 'Low-stock threshold',
      data: props.history.map(() => props.lowStockThreshold as number),
      borderColor: 'rgba(248, 113, 113, 0.7)',
      borderDash: [4, 4],
      borderWidth: 1,
      pointRadius: 0,
      pointHoverRadius: 0,
      fill: false,
    })
  }
  return {
    labels: props.history.map(([ts]) => formatLabel(ts)),
    datasets,
  }
})

const chartOptions = computed<ChartOptions<'line'>>(() => ({
  responsive: true,
  maintainAspectRatio: false,
  interaction: { mode: 'index', intersect: false },
  plugins: {
    legend: { display: false },
    tooltip: {
      backgroundColor: '#1a2035',
      borderColor: 'rgba(139, 92, 246, 0.4)',
      borderWidth: 1,
      titleColor: '#e2e8f0',
      bodyColor: '#c4cdd6',
      padding: 10,
      displayColors: false,
      callbacks: {
        title: (items) => items[0]?.label ?? '',
        label: (item) => ` ${item.dataset.label}: ${item.parsed.y}`,
      },
    },
  },
  scales: {
    x: {
      grid: { display: false },
      ticks: { color: 'rgba(196, 205, 214, 0.5)', maxTicksLimit: 4, font: { size: 9 } },
    },
    y: {
      beginAtZero: true,
      grid: { color: 'rgba(196, 205, 214, 0.08)' },
      ticks: { color: 'rgba(196, 205, 214, 0.5)', precision: 0, maxTicksLimit: 4, font: { size: 9 } },
    },
  },
}))
</script>

<template>
  <div class="h-14 w-44">
    <Line :data="chartData" :options="chartOptions" />
  </div>
</template>
