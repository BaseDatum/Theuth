// 224: "Region · nearest picked for you". The browser's time zone stands in for where the user is (no location
// permission, nothing sent anywhere); the provider region with the shortest great-circle distance wins.

import type { Region } from './types'

type LatLon = readonly [number, number]

/** The cities behind every catalog region (`crates/cloudnodes/src/providers/catalog.rs`), by label. */
const REGION_AT: Record<string, LatLon> = {
  'New York': [40.7, -74.0],
  'San Francisco': [37.8, -122.4],
  Toronto: [43.7, -79.4],
  London: [51.5, -0.1],
  Amsterdam: [52.4, 4.9],
  Frankfurt: [50.1, 8.7],
  Bangalore: [12.97, 77.6],
  Singapore: [1.35, 103.8],
  Sydney: [-33.9, 151.2],
  'Ashburn, Virginia': [39.0, -77.5],
  Chicago: [41.9, -87.6],
  'San Jose': [37.3, -121.9],
  Tokyo: [35.7, 139.7],
  'São Paulo': [-23.5, -46.6],
  Virginia: [38.9, -77.4],
  Ohio: [40.0, -83.0],
  Oregon: [45.5, -122.7],
  Montreal: [45.5, -73.6],
  Dublin: [53.3, -6.3],
  Mumbai: [19.1, 72.9],
}

/** Common IANA zones by their city; the rest fall back to their area (`Europe/…`). */
const ZONE_AT: Record<string, LatLon> = {
  'Europe/London': [51.5, -0.1],
  'Europe/Dublin': [53.3, -6.3],
  'Europe/Lisbon': [38.7, -9.1],
  'Europe/Madrid': [40.4, -3.7],
  'Europe/Paris': [48.9, 2.35],
  'Europe/Brussels': [50.8, 4.35],
  'Europe/Amsterdam': [52.4, 4.9],
  'Europe/Berlin': [52.5, 13.4],
  'Europe/Zurich': [47.4, 8.5],
  'Europe/Rome': [41.9, 12.5],
  'Europe/Vienna': [48.2, 16.4],
  'Europe/Prague': [50.1, 14.4],
  'Europe/Warsaw': [52.2, 21.0],
  'Europe/Stockholm': [59.3, 18.1],
  'Europe/Oslo': [59.9, 10.75],
  'Europe/Copenhagen': [55.7, 12.6],
  'Europe/Helsinki': [60.2, 24.9],
  'Europe/Athens': [38.0, 23.7],
  'Europe/Istanbul': [41.0, 29.0],
  'Europe/Kyiv': [50.45, 30.5],
  'Europe/Kiev': [50.45, 30.5],
  'Europe/Moscow': [55.75, 37.6],
  'America/New_York': [40.7, -74.0],
  'America/Detroit': [42.3, -83.0],
  'America/Toronto': [43.7, -79.4],
  'America/Montreal': [45.5, -73.6],
  'America/Halifax': [44.6, -63.6],
  'America/Chicago': [41.9, -87.6],
  'America/Denver': [39.7, -105.0],
  'America/Phoenix': [33.4, -112.0],
  'America/Los_Angeles': [34.0, -118.2],
  'America/Vancouver': [49.3, -123.1],
  'America/Anchorage': [61.2, -149.9],
  'America/Mexico_City': [19.4, -99.1],
  'America/Bogota': [4.7, -74.0],
  'America/Lima': [-12.0, -77.0],
  'America/Santiago': [-33.4, -70.7],
  'America/Sao_Paulo': [-23.5, -46.6],
  'America/Argentina/Buenos_Aires': [-34.6, -58.4],
  'Pacific/Honolulu': [21.3, -157.9],
  'Pacific/Auckland': [-36.85, 174.8],
  'Asia/Tokyo': [35.7, 139.7],
  'Asia/Seoul': [37.6, 127.0],
  'Asia/Shanghai': [31.2, 121.5],
  'Asia/Hong_Kong': [22.3, 114.2],
  'Asia/Taipei': [25.0, 121.5],
  'Asia/Manila': [14.6, 121.0],
  'Asia/Singapore': [1.35, 103.8],
  'Asia/Kuala_Lumpur': [3.1, 101.7],
  'Asia/Jakarta': [-6.2, 106.8],
  'Asia/Bangkok': [13.75, 100.5],
  'Asia/Kolkata': [22.6, 88.4],
  'Asia/Calcutta': [22.6, 88.4],
  'Asia/Karachi': [24.9, 67.0],
  'Asia/Dubai': [25.2, 55.3],
  'Asia/Jerusalem': [31.8, 35.2],
  'Australia/Sydney': [-33.9, 151.2],
  'Australia/Melbourne': [-37.8, 145.0],
  'Australia/Brisbane': [-27.5, 153.0],
  'Australia/Adelaide': [-34.9, 138.6],
  'Australia/Perth': [-31.95, 115.9],
  'Africa/Johannesburg': [-26.2, 28.0],
  'Africa/Lagos': [6.5, 3.4],
  'Africa/Cairo': [30.0, 31.2],
  'Africa/Nairobi': [-1.3, 36.8],
}

const AREA_AT: Record<string, LatLon> = {
  Europe: [50.0, 10.0],
  America: [39.0, -95.0],
  Asia: [25.0, 100.0],
  Australia: [-33.0, 147.0],
  Pacific: [-20.0, 175.0],
  Africa: [5.0, 20.0],
  Atlantic: [40.0, -20.0],
  Indian: [-10.0, 75.0],
}

export function zoneAt(zone: string): LatLon | null {
  return ZONE_AT[zone] ?? AREA_AT[zone.split('/')[0] ?? ''] ?? null
}

function km([a, b]: LatLon, [c, d]: LatLon): number {
  const r = Math.PI / 180
  const h = Math.sin(((c - a) * r) / 2) ** 2 + Math.cos(a * r) * Math.cos(c * r) * Math.sin(((d - b) * r) / 2) ** 2
  return 12_742 * Math.asin(Math.sqrt(h))
}

/** This device's IANA zone (`Europe/London`), or `''` where `Intl` cannot say. */
export function localZone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone ?? ''
  } catch {
    return ''
  }
}

/** The region of `regions` nearest to `zone`; the first one when the zone (UTC, an unknown name) places nobody. */
export function nearestRegion(regions: readonly Region[], zone = localZone()): Region | null {
  const here = zoneAt(zone)
  if (!here) return regions[0] ?? null
  let best: Region | null = null
  let bestKm = Number.POSITIVE_INFINITY
  for (const region of regions) {
    const at = REGION_AT[region.label]
    if (!at) continue
    const d = km(here, at)
    if (d < bestKm) [best, bestKm] = [region, d]
  }
  return best ?? regions[0] ?? null
}
