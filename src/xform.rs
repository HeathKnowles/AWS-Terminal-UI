/// Port of botocore's `xform_name` (awscli/botocore/__init__.py:182).
///
/// Converts camel-case names to kebab-case CLI names, matching exactly what
/// the aws CLI produces for operation and parameter names.
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;
use std::sync::Mutex;

// _first_cap_regex = re.compile('(.)([A-Z][a-z]+)')
static FIRST_CAP: Lazy<Regex> = Lazy::new(|| Regex::new(r"(.)([A-Z][a-z]+)").unwrap());
// _end_cap_regex = re.compile('([a-z0-9])([A-Z])')
static END_CAP: Lazy<Regex> = Lazy::new(|| Regex::new(r"([a-z0-9])([A-Z])").unwrap());
// _special_case_transform = re.compile('[A-Z]{2,}s$')
static SPECIAL_CASE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[A-Z]{2,}s$").unwrap());

// Prepopulated cache matching `_xform_cache` in botocore/__init__.py.
// These special cases don't match the regular transformation.
static XFORM_CACHE_OVERRIDES: &[(&str, &str)] = &[
    ("AssociateWhatsAppBusinessAccount", "associate-whatsapp-business-account"),
    ("CreateCachediSCSIVolume", "create-cached-iscsi-volume"),
    ("CreateStorediSCSIVolume", "create-stored-iscsi-volume"),
    ("CreateWhatsAppDataset", "create-whatsapp-dataset"),
    ("CreateWhatsAppFlow", "create-whatsapp-flow"),
    ("CreateWhatsAppMessageTemplate", "create-whatsapp-message-template"),
    ("CreateWhatsAppMessageTemplateFromLibrary", "create-whatsapp-message-template-from-library"),
    ("CreateWhatsAppMessageTemplateMedia", "create-whatsapp-message-template-media"),
    ("DeleteWhatsAppFlow", "delete-whatsapp-flow"),
    ("DeleteWhatsAppMessageMedia", "delete-whatsapp-message-media"),
    ("DeleteWhatsAppMessageTemplate", "delete-whatsapp-message-template"),
    ("DeprecateWhatsAppFlow", "deprecate-whatsapp-flow"),
    ("DescribeCachediSCSIVolumes", "describe-cached-iscsi-volumes"),
    ("DescribeStorediSCSIVolumes", "describe-stored-iscsi-volumes"),
    ("DisassociateWhatsAppBusinessAccount", "disassociate-whatsapp-business-account"),
    ("ExecutePartiQLBatch", "execute-partiql-batch"),
    ("ExecutePartiQLStatement", "execute-partiql-statement"),
    ("ExecutePartiQLTransaction", "execute-partiql-transaction"),
    ("GetLinkedWhatsAppBusinessAccount", "get-linked-whatsapp-business-account"),
    ("GetLinkedWhatsAppBusinessAccountPhoneNumber", "get-linked-whatsapp-business-account-phone-number"),
    ("GetWhatsAppFlow", "get-whatsapp-flow"),
    ("GetWhatsAppFlowPreview", "get-whatsapp-flow-preview"),
    ("GetWhatsAppMessageMedia", "get-whatsapp-message-media"),
    ("GetWhatsAppMessageTemplate", "get-whatsapp-message-template"),
    ("ListHITsForQualificationType", "list-hits-for-qualification-type"),
    ("ListLinkedWhatsAppBusinessAccounts", "list-linked-whatsapp-business-accounts"),
    ("ListWhatsAppFlowAssets", "list-whatsapp-flow-assets"),
    ("ListWhatsAppFlows", "list-whatsapp-flows"),
    ("ListWhatsAppMessageTemplates", "list-whatsapp-message-templates"),
    ("ListWhatsAppTemplateLibrary", "list-whatsapp-template-library"),
    ("PostWhatsAppMessageMedia", "post-whatsapp-message-media"),
    ("PublishWhatsAppFlow", "publish-whatsapp-flow"),
    ("PutWhatsAppBusinessAccountEventDestinations", "put-whatsapp-business-account-event-destinations"),
    ("SendWhatsAppConversionEvent", "send-whatsapp-conversion-event"),
    ("SendWhatsAppMessage", "send-whatsapp-message"),
    ("UpdateWhatsAppFlow", "update-whatsapp-flow"),
    ("UpdateWhatsAppFlowAssets", "update-whatsapp-flow-assets"),
    ("UpdateWhatsAppMessageTemplate", "update-whatsapp-message-template"),
];

static XFORM_CACHE: Lazy<Mutex<HashMap<(String, char), String>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub fn xform_name(name: &str, sep: char) -> String {
    if name.contains(sep) {
        return name.to_string();
    }
    let key = (name.to_string(), sep);
    if let Ok(cache) = XFORM_CACHE.lock() {
        if let Some(v) = cache.get(&key) {
            return v.clone();
        }
    }
    let transformed = xform_name_uncached(name, sep);
    if let Ok(mut cache) = XFORM_CACHE.lock() {
        cache.insert(key, transformed.clone());
    }
    transformed
}

fn xform_name_uncached(name: &str, sep: char) -> String {
    let sep = sep.to_string();
    if let Some((special, val)) = XFORM_CACHE_OVERRIDES
        .iter()
        .find(|(n, _)| *n == name)
    {
        let _ = special;
        return if sep == "-" {
            val.to_string()
        } else {
            val.replace('-', "_")
        };
    }
    let mut name = name.to_string();
    // Special case: trailing pluralized acronym -> _arns, _acls, etc.
    if let Some(m) = SPECIAL_CASE.find(&name) {
        let matched = m.as_str().to_ascii_lowercase();
        let start = m.start();
        name = format!("{}{}{}", &name[..start], sep, matched);
    }
    let s1 = FIRST_CAP
        .replace_all(&name, format!("$1{sep}$2"))
        .to_string();
    let transformed = END_CAP
        .replace_all(&s1, format!("$1{sep}$2"))
        .to_ascii_lowercase();
    transformed
}

#[cfg(test)]
mod tests {
    use super::xform_name;

    #[test]
    fn basic_camel_case() {
        assert_eq!(xform_name("DescribeInstances", '-'), "describe-instances");
        assert_eq!(xform_name("RunInstances", '-'), "run-instances");
        assert_eq!(xform_name("ListObjectsV2", '-'), "list-objects-v2");
        assert_eq!(xform_name("CreateStackSet", '-'), "create-stack-set");
    }

    #[test]
    fn already_separated() {
        assert_eq!(xform_name("list-objects-v2", '-'), "list-objects-v2");
    }

    #[test]
    fn pluralized_acronyms() {
        assert_eq!(xform_name("ListWebACLs", '-'), "list-web-acls");
        assert_eq!(xform_name("GetGatewayARNs", '-'), "get-gateway-arns");
    }

    #[test]
    fn cached_special_cases() {
        assert_eq!(xform_name("CreateCachediSCSIVolume", '-'), "create-cached-iscsi-volume");
        assert_eq!(xform_name("ExecutePartiQLStatement", '-'), "execute-partiql-statement");
        assert_eq!(xform_name("ListHITsForQualificationType", '-'), "list-hits-for-qualification-type");
        assert_eq!(xform_name("GetWhatsAppMessageMedia", '-'), "get-whatsapp-message-media");
    }

    #[test]
    fn cross_check_against_real_models() {
        // Verify against a few operations actually present in the models.
        assert_eq!(xform_name("DescribeSubnets", '-'), "describe-subnets");
        assert_eq!(xform_name("DescribeAvailabilityZones", '-'), "describe-availability-zones");
        assert_eq!(xform_name("GetCallerIdentity", '-'), "get-caller-identity");
        assert_eq!(xform_name("ListBuckets", '-'), "list-buckets");
        assert_eq!(xform_name("DescribeVpcs", '-'), "describe-vpcs");
    }
}
