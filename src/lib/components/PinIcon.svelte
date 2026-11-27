<!-- The pin of the Pull requests page (its rows, a pull request's page): filled when pinned. -->
<script lang="ts">
  type Props = { on?: boolean; size?: number };
  let { on = false, size = 14 }: Props = $props();
</script>

<svg width={size} height={size} viewBox="0 0 16 16" aria-hidden="true" style="display:block">
  <path
    d="M9.6 2.2 13.8 6.4l-1.2.6-2.1 2.1.1 2.9-1.2 1.2-2.6-2.6-3.3 3.3M5.1 7.1 7.9 4.3l.6-1.2z"
    fill={on ? "currentColor" : "none"}
    stroke="currentColor"
    stroke-width="1.3"
    stroke-linejoin="round"
    stroke-linecap="round"
  />
</svg>
