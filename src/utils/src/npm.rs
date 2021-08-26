use crate::constants::MAX_RETRIES;
use crate::errors::VoltError;
use crate::volt_api::VoltPackage;
use colored::Colorize;
use futures::stream::FuturesOrdered;
use futures::TryStreamExt;
use isahc::http::StatusCode;
use isahc::AsyncReadResponseExt;
use isahc::Request;
use isahc::RequestExt;
use miette::DiagnosticResult;
use semver_rs::Version;
use serde_json::Value;
use ssri::{Algorithm, Integrity};

// Get version from NPM
pub async fn get_version(
    package_name: String,
) -> DiagnosticResult<(String, String, String, Option<VoltPackage>)> {
    let mut retries = 0;

    let count = package_name.matches("@").count();

    if (count == 1 && package_name.contains("/")) || (count == 0 && !package_name.contains("/")) {
        loop {
            let client: Request<&str> =
                Request::get(format!("http://registry.npmjs.org/{}", package_name))
                    .header(
                        "Accept",
                        "application/vnd.npm.install-v1+json; q=1.0, application/json; q=0.8, */*",
                    )
                    .body("")
                    .map_err(VoltError::RequestBuilderError)?;

            let mut response = client.send_async().await.map_err(VoltError::NetworkError)?;

            match response.status_mut() {
                &mut StatusCode::OK => {
                    let text = response.text().await.map_err(VoltError::IoTextRecError)?;

                    match serde_json::from_str::<Value>(&text).unwrap()["dist-tags"]["latest"]
                        .as_str()
                    {
                        Some(latest) => {
                            let num_deps;

                            match serde_json::from_str::<Value>(&text).unwrap()["versions"][latest]
                                ["dependencies"]
                                .as_object()
                            {
                                Some(value) => {
                                    num_deps = value.keys().count();
                                }
                                None => {
                                    num_deps = 0;
                                }
                            }

                            let mut package: Option<VoltPackage> = None;

                            match serde_json::from_str::<Value>(&text).unwrap()["versions"][latest]
                                ["dist"]
                                .as_object()
                            {
                                Some(value) => {
                                    let hash_string: String;

                                    if value.contains_key("integrity") {
                                        hash_string =
                                            value["integrity"].to_string().replace("\"", "");
                                    } else {
                                        hash_string = format!(
                                            "sha1-{}",
                                            base64::encode(value["shasum"].to_string())
                                        );
                                    }

                                    let integrity: Integrity =
                                        hash_string.parse().map_err(|_| {
                                            VoltError::HashParseError {
                                                hash: hash_string.to_string(),
                                            }
                                        })?;

                                    let algo = integrity.pick_algorithm();

                                    let mut hash = integrity
                                        .hashes
                                        .into_iter()
                                        .find(|h| h.algorithm == algo)
                                        .map(|h| Integrity { hashes: vec![h] })
                                        .map(|i| i.to_hex().1)
                                        .ok_or(VoltError::IntegrityConversionError)?;

                                    match algo {
                                        Algorithm::Sha1 => {
                                            hash = format!("sha1-{}", hash);
                                        }
                                        Algorithm::Sha512 => {
                                            hash = format!("sha512-{}", hash);
                                        }
                                        _ => {}
                                    }

                                    if num_deps == 0 {
                                        package = Some(VoltPackage {
                                            name: package_name.clone(),
                                            version: latest.to_string(),
                                            tarball: value["tarball"].to_string().replace("\"", ""),
                                            bin: None,
                                            integrity: hash.clone(),
                                            peer_dependencies: None,
                                            dependencies: None,
                                        })
                                    }

                                    return Ok((package_name, latest.to_string(), hash, package));
                                }
                                None => {
                                    return Err(VoltError::HashLookupError {
                                        version: latest.to_string(),
                                    })?;
                                }
                            }
                        }
                        None => {
                            return Err(VoltError::VersionLookupError { name: package_name })?;
                        }
                    }
                }
                &mut StatusCode::NOT_FOUND => {
                    if retries == MAX_RETRIES {
                        return Err(VoltError::TooManyRequests {
                            url: format!("http://registry.npmjs.org/{}", package_name),
                            package_name: package_name.to_string(),
                        })?;
                    }
                }
                _ => {
                    if retries == MAX_RETRIES {
                        return Err(VoltError::PackageNotFound {
                            url: format!("http://registry.npmjs.org/{}", package_name),
                            package_name: package_name.to_string(),
                        })?;
                    }
                }
            }

            retries += 1;
        }
    } else {
        if count == 2 && package_name.contains("/") {
            let input_version = package_name.split("@").collect::<Vec<&str>>()[2].to_string();

            let version_requirement = semver_rs::Range::new(&input_version).parse().unwrap();

            loop {
                let name = format!("@{}", input_version);

                let client: Request<&str> = Request::get(format!(
                    "http://registry.npmjs.org/{}",
                    package_name.replace(&name, "")
                ))
                .header(
                    "Accept",
                    "application/vnd.npm.install-v1+json; q=1.0, application/json; q=0.8, */*",
                )
                .body("")
                .map_err(VoltError::RequestBuilderError)?;

                let mut response = client.send_async().await.map_err(VoltError::NetworkError)?;

                match response.status_mut() {
                    &mut StatusCode::OK => {
                        let text = response.text().await.map_err(VoltError::IoTextRecError)?;

                        match serde_json::from_str::<Value>(&text).unwrap()["versions"].as_object()
                        {
                            Some(value) => {
                                let mut available_versions = value
                                    .keys()
                                    .filter_map(|k| Version::new(k).parse().ok())
                                    .filter(|v| version_requirement.test(&v))
                                    .collect::<Vec<_>>();

                                available_versions
                                    .sort_unstable_by(|a, b| a.partial_cmp(b).unwrap().reverse());

                                if available_versions.is_empty() {
                                    return Err(VoltError::VersionLookupError {
                                        name: package_name,
                                    })?;
                                }

                                let num_deps;

                                match serde_json::from_str::<Value>(&text).unwrap()["versions"]
                                    [available_versions[0].to_string()]["dependencies"]
                                    .as_object()
                                {
                                    Some(value) => {
                                        num_deps = value.keys().count();
                                    }
                                    None => {
                                        num_deps = 0;
                                    }
                                }

                                let mut package: Option<VoltPackage> = None;

                                match serde_json::from_str::<Value>(&text).unwrap()["versions"]
                                    [available_versions[0].to_string()]["dist"]
                                    .as_object()
                                {
                                    Some(value) => {
                                        let hash_string: String;

                                        if value.contains_key("integrity") {
                                            hash_string =
                                                value["integrity"].to_string().replace("\"", "");
                                        } else {
                                            hash_string = format!(
                                                "sha1-{}",
                                                base64::encode(value["shasum"].to_string())
                                            );
                                        }

                                        let integrity: Integrity =
                                            hash_string.parse().map_err(|_| {
                                                VoltError::HashParseError {
                                                    hash: hash_string.to_string(),
                                                }
                                            })?;

                                        let algo = integrity.pick_algorithm();

                                        let mut hash = integrity
                                            .hashes
                                            .into_iter()
                                            .find(|h| h.algorithm == algo)
                                            .map(|h| Integrity { hashes: vec![h] })
                                            .map(|i| i.to_hex().1)
                                            .ok_or(VoltError::IntegrityConversionError)?;

                                        match algo {
                                            Algorithm::Sha1 => {
                                                hash = format!("sha1-{}", hash);
                                            }
                                            Algorithm::Sha512 => {
                                                hash = format!("sha512-{}", hash);
                                            }
                                            _ => {}
                                        }

                                        if num_deps == 0 {
                                            package = Some(VoltPackage {
                                                name: package_name.replace(&name, ""),
                                                version: input_version,
                                                tarball: value["tarball"]
                                                    .to_string()
                                                    .replace("\"", ""),
                                                bin: None,
                                                integrity: hash.clone(),
                                                peer_dependencies: None,
                                                dependencies: None,
                                            })
                                        }
                                        return Ok((
                                            package_name,
                                            available_versions[0].to_string(),
                                            hash,
                                            package,
                                        ));
                                    }
                                    None => {
                                        return Err(VoltError::HashLookupError {
                                            version: available_versions[0].to_string(),
                                        })?;
                                    }
                                }
                            }
                            None => {
                                return Err(VoltError::VersionLookupError { name: package_name })?;
                            }
                        }
                    }
                    &mut StatusCode::NOT_FOUND => {
                        if retries == MAX_RETRIES {
                            return Err(VoltError::TooManyRequests {
                                url: format!("http://registry.npmjs.org/{}", package_name),
                                package_name: package_name.to_string(),
                            })?;
                        }
                    }
                    _ => {
                        return Err(VoltError::PackageNotFound {
                            url: format!("http://registry.npmjs.org/{}", package_name),
                            package_name: package_name.to_string(),
                        })?;
                    }
                }

                retries += 1;
            }
        } else if count == 1 && !package_name.contains("/") {
            let input_version = package_name.split("@").collect::<Vec<&str>>()[1].to_string();

            let version_requirement = semver_rs::Range::new(&input_version).parse().unwrap();

            loop {
                let name = format!("@{}", input_version);

                let client: Request<&str> = Request::get(format!(
                    "http://registry.npmjs.org/{}",
                    package_name.replace(&name, "")
                ))
                .header(
                    "Accept",
                    "application/vnd.npm.install-v1+json; q=1.0, application/json; q=0.8, */*",
                )
                .body("")
                .map_err(VoltError::RequestBuilderError)?;

                let mut response = client.send_async().await.map_err(VoltError::NetworkError)?;

                match response.status_mut() {
                    &mut StatusCode::OK => {
                        let text = response.text().await.map_err(VoltError::IoTextRecError)?;

                        match serde_json::from_str::<Value>(&text).unwrap()["versions"].as_object()
                        {
                            Some(value) => {
                                let mut available_versions = value
                                    .keys()
                                    .filter_map(|k| Version::new(k).parse().ok())
                                    .filter(|v| version_requirement.test(&v))
                                    .collect::<Vec<_>>();

                                available_versions
                                    .sort_unstable_by(|a, b| a.partial_cmp(b).unwrap().reverse());

                                if available_versions.is_empty() {
                                    return Err(VoltError::VersionLookupError {
                                        name: package_name,
                                    })?;
                                }

                                let num_deps;

                                match serde_json::from_str::<Value>(&text).unwrap()["versions"]
                                    [available_versions[0].to_string()]["dependencies"]
                                    .as_object()
                                {
                                    Some(value) => {
                                        num_deps = value.keys().count();
                                    }
                                    None => {
                                        num_deps = 0;
                                    }
                                }

                                let mut package: Option<VoltPackage> = None;

                                match serde_json::from_str::<Value>(&text).unwrap()["versions"]
                                    [available_versions[0].to_string()]["dist"]
                                    .as_object()
                                {
                                    Some(value) => {
                                        let hash_string: String;

                                        if value.contains_key("integrity") {
                                            hash_string =
                                                value["integrity"].to_string().replace("\"", "");
                                        } else {
                                            hash_string = format!(
                                                "sha1-{}",
                                                base64::encode(value["shasum"].to_string())
                                            );
                                        }

                                        let integrity: Integrity =
                                            hash_string.parse().map_err(|_| {
                                                VoltError::HashParseError {
                                                    hash: hash_string.to_string(),
                                                }
                                            })?;

                                        let algo = integrity.pick_algorithm();

                                        let mut hash = integrity
                                            .hashes
                                            .into_iter()
                                            .find(|h| h.algorithm == algo)
                                            .map(|h| Integrity { hashes: vec![h] })
                                            .map(|i| i.to_hex().1)
                                            .ok_or(VoltError::IntegrityConversionError)?;

                                        match algo {
                                            Algorithm::Sha1 => {
                                                hash = format!("sha1-{}", hash);
                                            }
                                            Algorithm::Sha512 => {
                                                hash = format!("sha512-{}", hash);
                                            }
                                            _ => {}
                                        }

                                        if num_deps == 0 {
                                            package = Some(VoltPackage {
                                                name: package_name.replace(&name, ""),
                                                version: input_version,
                                                tarball: value["tarball"]
                                                    .to_string()
                                                    .replace("\"", ""),
                                                bin: None,
                                                integrity: hash.clone(),
                                                peer_dependencies: None,
                                                dependencies: None,
                                            })
                                        }

                                        return Ok((
                                            package_name,
                                            available_versions[0].to_string(),
                                            hash,
                                            package,
                                        ));
                                    }
                                    None => {
                                        return Err(VoltError::HashLookupError {
                                            version: available_versions[0].to_string(),
                                        })?;
                                    }
                                }
                            }
                            None => {}
                        }
                    }
                    &mut StatusCode::NOT_FOUND => {
                        if retries == MAX_RETRIES {
                            return Err(VoltError::VersionLookupError { name: package_name })?;
                        }
                    }
                    _ => {
                        if retries == MAX_RETRIES {
                            if retries == MAX_RETRIES {
                                return Err(VoltError::PackageNotFound {
                                    url: format!("http://registry.npmjs.org/{}", package_name),
                                    package_name: package_name.to_string(),
                                })?;
                            }
                        }
                    }
                }

                retries += 1;
            }
        } else {
            return Err(VoltError::UnknownError)?;
        }
    }
}

pub async fn get_versions(
    packages: &Vec<String>,
) -> DiagnosticResult<Vec<(String, String, String, Option<VoltPackage>)>> {
    packages
        .to_owned()
        .into_iter()
        .map(get_version)
        .collect::<FuturesOrdered<_>>()
        .try_collect::<Vec<(String, String, String, Option<VoltPackage>)>>()
        .await
}
