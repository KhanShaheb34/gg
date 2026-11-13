<!-- The app's mark: a branch that leaves a line and merges back, in the first two lane colours. -->
<script lang="ts">
  let { size = 32 }: { size?: number } = $props();
</script>

<svg class="mark" viewBox="0 0 32 32" width={size} height={size} aria-hidden="true">
  <path class="line" d="M10 3V29" stroke="var(--lane-0)" />
  <path class="line" d="M10 24C10 19.5 22 20.5 22 16S10 12.5 10 8" stroke="var(--lane-1)" />
  <circle cx="10" cy="8" r="3.6" fill="var(--lane-0)" />
  <circle cx="22" cy="16" r="3.6" fill="var(--lane-1)" />
  <circle cx="10" cy="24" r="3.6" fill="var(--lane-0)" />
</svg>

<style>
  .mark {
    display: block;
    flex: none;
  }
  .line {
    fill: none;
    stroke-width: 2.5;
    stroke-linecap: round;
  }
  circle {
    stroke: var(--mark-ground, var(--bg));
    stroke-width: 2;
  }
</style>
