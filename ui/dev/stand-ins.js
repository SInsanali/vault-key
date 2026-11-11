// Stand-in weapon pictures for the browser preview. The real app draws each weapon from the
// player's own game files; nothing from the game may be kept in this repository, so the
// preview shows these plain silhouettes instead: original drawings, one per kind of weapon,
// with the muzzle to the left like the real pictures.

const SHAPES = {
  pistol: "M40 34h92l6 6v12h-22l12 36h-26l-10-30h-10v8h-10v-8H52l-12-6z",
  smg: "M22 40h22v-8h96v6h44v6h-40v10h-10l8 28h-22l-8-28h-18v30H80V54H44v-8H22z",
  "assault-rifle": "M8 40h44v-8h90l48 4v22l-46-6h-6l8 26h-20l-8-26h-16l-6 36-20-4 8-32H52v-6H8z",
  shotgun: "M8 34h106v-4h36l42 8v20l-40-6h-8l6 24h-18l-8-24h-10V44H84v12H38V44H8z",
  "sniper-rifle": "M4 41h104v-7h44l40 8v16l-38-6h-6l6 22h-16l-8-22h-22v-6H4zM92 20h50v9H92z",
  launcher: "M8 28h178v8l8 4v8l-8 4v6H120l6 26h-20l-6-26H8zM96 14h34v10H96z",
};

/**
 * A data: address of the stand-in for `item` (the preview's sample items), or null for
 * things the real app has no picture of either: items that are not weapons, and anything
 * the preview marks as unverified add-on content.
 */
export function picture(item) {
  const shape = item.isWeapon && !item.unverified ? SHAPES[item.category] : null;
  if (!shape) return null;
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 100">` +
    `<path d="${shape}" fill="#6B5A44" stroke="#ECDEC4" stroke-width="2.5" stroke-linejoin="round"/>` +
    `<path d="${shape}" fill="none" stroke="#000" stroke-width="1"/>` +
    `<text x="100" y="97" text-anchor="middle" font-family="sans-serif" font-size="9" font-weight="700" letter-spacing="2" fill="#A8998A">PREVIEW STAND-IN</text>` +
    `</svg>`;
  return `data:image/svg+xml,${encodeURIComponent(svg)}`;
}
