import Link from "@docusaurus/Link";
import { IconDownload } from "@site/src/components/icons/download";
import { useState, useEffect } from "react";

const FALLBACK_URL = "https://github.com/aaif-goose/goose/releases/latest";

const isStandardLinuxAsset = (asset) => !asset.name.includes('-vulkan');

const ARCH_ASSET_TOKENS = {
  x64: { deb: 'amd64', rpm: 'x86_64', flatpak: 'x86_64' },
  arm64: { deb: 'arm64', rpm: 'arm64', flatpak: 'aarch64' },
};

const detectLinuxArch = async () => {
  if (typeof navigator === 'undefined') return 'x64';

  try {
    if (navigator.userAgentData?.getHighEntropyValues) {
      const { architecture } = await navigator.userAgentData.getHighEntropyValues(['architecture']);
      if (architecture) return /arm/i.test(architecture) ? 'arm64' : 'x64';
    }
  } catch {
    // Fall back to the user agent.
  }

  return /aarch64|arm64/i.test(navigator.userAgent || '') ? 'arm64' : 'x64';
};

const LinuxDesktopInstallButtons = () => {
  const [downloadUrls, setDownloadUrls] = useState({
    deb: FALLBACK_URL,
    rpm: FALLBACK_URL,
    flatpak: FALLBACK_URL
  });

  useEffect(() => {
    const fetchLatestRelease = async () => {
      try {
        const arch = await detectLinuxArch();
        const tokens = ARCH_ASSET_TOKENS[arch];
        const cacheKey = `goose-release-cache-${arch}`;
        const cacheTimeKey = `goose-release-cache-time-${arch}`;

        // Check cache first (1 hour expiry)
        const cached = localStorage.getItem(cacheKey);
        const cacheTime = localStorage.getItem(cacheTimeKey);
        const now = Date.now();

        if (cached && cacheTime && (now - parseInt(cacheTime)) < 3600000) {
          // Use cached data if less than 1 hour old
          setDownloadUrls(JSON.parse(cached));
          return;
        }

        // Fetch latest release from GitHub API
        const response = await fetch('https://api.github.com/repos/aaif-goose/goose/releases/latest');
        if (!response.ok) throw new Error('API request failed');

        const release = await response.json();
        const assets = release.assets || [];

        // Find DEB, RPM, and Flatpak files
        const debAsset = assets.find(asset =>
          isStandardLinuxAsset(asset) && asset.name.includes('.deb') && asset.name.includes(tokens.deb)
        );
        const rpmAsset = assets.find(asset =>
          isStandardLinuxAsset(asset) && asset.name.includes('.rpm') && asset.name.includes(tokens.rpm)
        );
        const flatpakAsset = assets.find(asset =>
          isStandardLinuxAsset(asset) && asset.name.endsWith('.flatpak') && asset.name.includes(tokens.flatpak)
        );

        const newUrls = {
          deb: debAsset?.browser_download_url || FALLBACK_URL,
          rpm: rpmAsset?.browser_download_url || FALLBACK_URL,
          flatpak: flatpakAsset?.browser_download_url || FALLBACK_URL
        };

        // Update state and cache
        setDownloadUrls(newUrls);
        localStorage.setItem(cacheKey, JSON.stringify(newUrls));
        localStorage.setItem(cacheTimeKey, now.toString());
      } catch (error) {
        console.warn('Failed to fetch latest release, using fallback URLs:', error);
        // Fallback URLs are already set in initial state
      }
    };

    fetchLatestRelease();
  }, []);

  return (
    <div>
      <p>Click one of the buttons below to download goose Desktop for Linux:</p>
      <div className="pill-button" style={{ display: 'flex', gap: '0.5rem', flexWrap: 'wrap' }}>
        <Link
          className="button button--primary button--lg"
          to={downloadUrls.deb}
        >
          <IconDownload /> DEB Package (Ubuntu/Debian)
        </Link>
        <Link
          className="button button--primary button--lg"
          to={downloadUrls.rpm}
        >
          <IconDownload /> RPM Package (RHEL/Fedora)
        </Link>
        <Link
          className="button button--primary button--lg"
          to={downloadUrls.flatpak}
        >
          <IconDownload /> Flatpak (Universal)
        </Link>
      </div>
    </div>
  );
};

export default LinuxDesktopInstallButtons;
