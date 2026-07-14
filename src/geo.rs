pub fn geo_for(region: &str) -> &'static str {
    match region {
        "us-east4" | "us-east-1" | "nyc" | "ewr" | "ewr2" | "pit" | "pit1" => "us-east",
        "us-west2" | "us-west-1" | "lax" | "lax1" => "us-west",
        "europe-west3" | "eu-central-1" | "fra" | "fra2" | "europe-west4" | "ams" | "ams3" => {
            "eu-central"
        }
        "europe-west2" | "eu-west-2" | "eu-west-1" | "lon" | "lon1" => "eu-west",
        "asia-northeast1" | "ap-northeast-1" | "tyo" | "tyo2" => "ap-northeast",
        "asia-southeast1" | "ap-southeast-1" | "sgp" | "sgp2" => "ap-southeast",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clusters_span_providers() {
        assert_eq!(geo_for("us-east4"), "us-east");
        assert_eq!(geo_for("us-east-1"), "us-east");
        assert_eq!(geo_for("nyc"), "us-east");
        assert_eq!(geo_for("ewr2"), "us-east");
        assert_eq!(geo_for("pit1"), "us-east");
        assert_eq!(geo_for("lax1"), "us-west");
        assert_eq!(geo_for("fra2"), "eu-central");
        assert_eq!(geo_for("ams"), "eu-central");
        assert_eq!(geo_for("lon1"), "eu-west");
        assert_eq!(geo_for("eu-west-1"), "eu-west");
        assert_eq!(geo_for("tyo2"), "ap-northeast");
        assert_eq!(geo_for("sgp"), "ap-southeast");
        assert_eq!(geo_for("mars-1"), "unknown");
    }
}
