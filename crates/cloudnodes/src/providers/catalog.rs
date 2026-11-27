//! Sizes, regions and list prices per provider (USD per month). 2 GB is the minimum and default
//! everywhere; every node gets a `/data` disk billed on top of the server.

use crate::types::{ProviderKind, Quote, Region, SizeOption, SIZES_GB};

use super::{CloudError, Result};

/// The release manifest a VM node installs from.
pub const MANIFEST: &str = "https://s3.rwx.dev/theuth/releases/latest.json";
/// The engine image container providers run.
pub const ENGINE_IMAGE: &str = "ghcr.io/basedatum/theuth/engine:latest";

/// DigitalOcean Block Storage list price, USD per GB-month. The only price in this crate: DO's
/// API has no price field for volumes (https://docs.digitalocean.com/products/volumes/details/pricing/).
/// Server prices come live from each provider's API (`DigitalOcean::sizes`).
pub const DO_VOLUME_USD_PER_GB_MONTH: f64 = 0.10;

/// `(memory_gb, vcpus, slug, memory_mb)`.
type Row = (u8, u8, &'static str, u32);

const DO_SIZES: [Row; 3] = [
    (2, 1, "s-1vcpu-2gb", 2048),
    (4, 2, "s-2vcpu-4gb", 4096),
    (8, 4, "s-4vcpu-8gb", 8192),
];
const FLY_SIZES: [Row; 3] = [
    (2, 1, "shared-cpu-1x", 2048),
    (4, 2, "shared-cpu-2x", 4096),
    (8, 4, "shared-cpu-4x", 8192),
];
const K8S_SIZES: [Row; 3] = [(2, 1, "2Gi", 2048), (4, 1, "4Gi", 4096), (8, 1, "8Gi", 8192)];
const LIGHTSAIL_SIZES: [Row; 3] = [
    (2, 2, "small_3_0", 2048),
    (4, 2, "medium_3_0", 4096),
    (8, 2, "large_3_0", 8192),
];

fn rows(kind: ProviderKind) -> &'static [Row; 3] {
    match kind {
        ProviderKind::Digitalocean => &DO_SIZES,
        ProviderKind::Fly => &FLY_SIZES,
        ProviderKind::Kubernetes => &K8S_SIZES,
        ProviderKind::Lightsail => &LIGHTSAIL_SIZES,
    }
}

fn cents(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// The size skeleton (slug, RAM, vCPUs) with no price: DigitalOcean's come from its API.
pub fn sizes(kind: ProviderKind) -> Vec<SizeOption> {
    rows(kind)
        .iter()
        .map(|&(memory_gb, vcpus, slug, _)| SizeOption {
            memory_gb,
            vcpus,
            slug: slug.into(),
            monthly_usd: None,
        })
        .collect()
}

fn row(kind: ProviderKind, memory_gb: u8) -> Result<&'static Row> {
    rows(kind)
        .iter()
        .find(|r| r.0 == memory_gb)
        .ok_or_else(|| CloudError(format!("size {memory_gb} GB: one of {SIZES_GB:?} GB")))
}

/// The provider's slug for `memory_gb`.
pub fn slug(kind: ProviderKind, memory_gb: u8) -> Result<String> {
    Ok(row(kind, memory_gb)?.2.to_owned())
}

/// `(vcpus, memory_mb)` for `memory_gb` (Fly's machine guest).
pub fn guest(kind: ProviderKind, memory_gb: u8) -> Result<(u8, u32)> {
    let r = row(kind, memory_gb)?;
    Ok((r.1, r.3))
}

/// The memory a slug stands for (`s-2vcpu-4gb` → 4); `None` for a size this catalog lacks.
pub fn memory_of(kind: ProviderKind, slug: &str) -> Option<u8> {
    rows(kind).iter().find(|r| r.2 == slug).map(|r| r.0)
}

/// Disk price per GB-month: DigitalOcean's list price; `None` elsewhere (not machine-readable).
pub fn volume_usd_per_gb(kind: ProviderKind) -> Option<f64> {
    (kind == ProviderKind::Digitalocean).then_some(DO_VOLUME_USD_PER_GB_MONTH)
}

/// The cost of `memory_gb` + a `volume_gb` disk, priced from `sizes` (the provider's live list;
/// a size without a price gives no server/total).
pub fn quote(kind: ProviderKind, sizes: &[SizeOption], memory_gb: u8, volume_gb: u32) -> Result<Quote> {
    if !(1..=16_384).contains(&volume_gb) {
        return Err(CloudError(format!("disk {volume_gb} GB: 1-16384 GB")));
    }
    let slug = slug(kind, memory_gb)?;
    let server = sizes
        .iter()
        .find(|s| s.slug == slug)
        .and_then(|s| s.monthly_usd);
    let disk = volume_usd_per_gb(kind).map(|p| cents(p * f64::from(volume_gb)));
    let total = match (server, disk) {
        (Some(s), Some(d)) => Some(cents(s + d)),
        _ => None,
    };
    Ok(Quote {
        provider: kind,
        size_slug: slug,
        server_usd: server,
        disk_usd: disk,
        total_usd: total,
    })
}

/// The regions a node can be created in (none for Kubernetes).
pub fn regions(kind: ProviderKind) -> Vec<Region> {
    let list: &[(&str, &str, &str)] = match kind {
        ProviderKind::Digitalocean => &[
            ("nyc3", "New York", "US"),
            ("sfo3", "San Francisco", "US"),
            ("tor1", "Toronto", "CA"),
            ("lon1", "London", "GB"),
            ("ams3", "Amsterdam", "NL"),
            ("fra1", "Frankfurt", "DE"),
            ("blr1", "Bangalore", "IN"),
            ("sgp1", "Singapore", "SG"),
            ("syd1", "Sydney", "AU"),
        ],
        ProviderKind::Fly => &[
            ("lhr", "London", "GB"),
            ("ams", "Amsterdam", "NL"),
            ("fra", "Frankfurt", "DE"),
            ("iad", "Ashburn, Virginia", "US"),
            ("ord", "Chicago", "US"),
            ("sjc", "San Jose", "US"),
            ("yyz", "Toronto", "CA"),
            ("sin", "Singapore", "SG"),
            ("syd", "Sydney", "AU"),
            ("nrt", "Tokyo", "JP"),
            ("gru", "São Paulo", "BR"),
        ],
        ProviderKind::Lightsail => &[
            ("us-east-1", "Virginia", "US"),
            ("us-east-2", "Ohio", "US"),
            ("us-west-2", "Oregon", "US"),
            ("ca-central-1", "Montreal", "CA"),
            ("eu-west-2", "London", "GB"),
            ("eu-west-1", "Dublin", "IE"),
            ("eu-central-1", "Frankfurt", "DE"),
            ("ap-south-1", "Mumbai", "IN"),
            ("ap-southeast-1", "Singapore", "SG"),
            ("ap-southeast-2", "Sydney", "AU"),
            ("ap-northeast-1", "Tokyo", "JP"),
        ],
        ProviderKind::Kubernetes => &[],
    };
    list.iter()
        .map(|&(id, label, country)| Region {
            id: id.into(),
            label: label.into(),
            country: country.into(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes() {
        let mut live = sizes(ProviderKind::Digitalocean);
        live[0].monthly_usd = Some(12.0);
        let q = quote(ProviderKind::Digitalocean, &live, 2, 10).unwrap();
        assert_eq!(q.size_slug, "s-1vcpu-2gb");
        assert_eq!((q.server_usd, q.disk_usd, q.total_usd), (Some(12.0), Some(1.0), Some(13.0)));
        // No live price: the disk only.
        let q = quote(ProviderKind::Digitalocean, &live, 4, 25).unwrap();
        assert_eq!((q.server_usd, q.disk_usd, q.total_usd), (None, Some(2.5), None));
        let f = quote(ProviderKind::Fly, &sizes(ProviderKind::Fly), 2, 10).unwrap();
        assert_eq!((f.size_slug.as_str(), f.server_usd, f.disk_usd, f.total_usd), ("shared-cpu-1x", None, None, None));
        let k = quote(ProviderKind::Kubernetes, &[], 2, 10).unwrap();
        assert_eq!((k.size_slug.as_str(), k.total_usd), ("2Gi", None));
        assert!(quote(ProviderKind::Digitalocean, &live, 1, 10).is_err());
        assert!(quote(ProviderKind::Digitalocean, &live, 2, 0).is_err());
        assert!(sizes(ProviderKind::Lightsail).iter().all(|s| s.monthly_usd.is_none()));
    }

    #[test]
    fn slugs_round_trip_and_regions_exist() {
        for kind in [
            ProviderKind::Digitalocean,
            ProviderKind::Fly,
            ProviderKind::Kubernetes,
            ProviderKind::Lightsail,
        ] {
            for gb in SIZES_GB {
                assert_eq!(memory_of(kind, &slug(kind, gb).unwrap()), Some(gb));
            }
            assert_eq!(sizes(kind)[0].memory_gb, 2);
        }
        assert_eq!(guest(ProviderKind::Fly, 2).unwrap(), (1, 2048));
        assert!(regions(ProviderKind::Digitalocean).iter().any(|r| r.id == "lon1" && r.country == "GB"));
        assert_eq!(regions(ProviderKind::Fly).len(), 11);
        assert!(regions(ProviderKind::Kubernetes).is_empty());
    }
}
