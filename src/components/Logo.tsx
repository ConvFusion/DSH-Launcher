// Brand logo — renders the app icon (public/icon1024.png).
export default function Logo({ size = 72 }: { size?: number }) {
  return (
    <img
      src="/icon1024.png"
      alt="DSH Launcher"
      width={size}
      height={size}
      style={{ borderRadius: size * 0.22, display: "block" }}
      draggable={false}
    />
  );
}
