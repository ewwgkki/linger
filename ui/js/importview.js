// Pure helpers for the Spotify import report.
import { t } from './i18n.js';

/** Plain text of the songs that were not found, one block per playlist (empty when nothing is missing). */
export function missingText(report) {
  return report.playlists
    .filter((p) => p.missing.length)
    .map((p) => [t('import.listHead', { name: p.name, matched: p.matched, total: p.total }), ...p.missing.map((m) => `${m.t} - ${m.a}${m.b ? ' · ' + m.b : ''}`)].join('\n'))
    .join('\n\n');
}

/** i18n key and params of the one-line summary: playlists actually created, songs matched / total. */
export function summaryKey(report) {
  const ps = report.playlists;
  return ['import.summary', {
    n: ps.filter((p) => p.id != null).length,
    matched: ps.reduce((a, p) => a + p.matched, 0),
    total: ps.reduce((a, p) => a + p.total, 0),
  }];
}
