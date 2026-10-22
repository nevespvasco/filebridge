import { useI18n } from "vue-i18n";
import { bcp47 } from "@/i18n";
import * as f from "@/lib/format";

/** Formatters bound to the current UI language. */
export function useFmt() {
  const { locale } = useI18n();
  const tag = () => bcp47(locale.value);
  return {
    bytes: (n: number | null | undefined) => f.formatBytes(n, tag()),
    rate: (n: number | null | undefined) => f.formatRate(n, tag()),
    count: (n: number | null | undefined) => f.formatCount(n, tag()),
    date: (ms: number | null | undefined) => f.formatDate(ms, tag()),
    time: (ms: number) => f.formatTime(ms, tag()),
    duration: f.formatDuration,
    percent: f.percent,
  };
}
