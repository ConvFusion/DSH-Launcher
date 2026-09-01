// Brand logo — reuses the master app icon (src-tauri/app-icon.png, 1024px)
// so the repo carries a single copy of the artwork. Vite bundles it into
// the web build as a static asset.
import logoUrl from "../../src-tauri/app-icon.png";

export default function Logo({ size = 72 }: { size?: number }) {
  return (
    <img
      src={logoUrl}
      alt="DSH Launcher"
      width={size}
      height={size}
      style={{ borderRadius: size * 0.22, display: "block" }}
      draggable={false}
    />
  );
}
