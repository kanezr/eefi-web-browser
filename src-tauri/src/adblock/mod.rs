use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock, RwLock,
    },
    time::{Duration, SystemTime},
};

use adblock::{
    cosmetic_filter_cache::ProceduralOrActionFilter,
    filters::cosmetic::{CosmeticFilterAction, CosmeticFilterOperator},
    lists::ParseOptions,
    request::Request,
    resources::Resource,
    Engine, FilterSet,
};
use futures_util::StreamExt;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::{State, Webview};
use url::Url;

use crate::privacy::{permission_origin_for_url, PrivacyService};
use crate::storage::StorageService;

const BUILTIN_FILTERS: &str = "\
||doubleclick.net^\n\
||googlesyndication.com^\n\
||google-analytics.com^\n\
||adservice.google.com^\n\
||ads-twitter.com^\n";
const MALWARE_FILTER_LIST: (&str, &str) = (
    "urlhaus-domains.txt",
    "https://malware-filter.gitlab.io/malware-filter/urlhaus-filter-domains.txt",
);
const FILTER_LISTS: [(&str, &str); 4] = [
    ("easylist.txt", "https://easylist.to/easylist/easylist.txt"),
    (
        "easyprivacy.txt",
        "https://easylist.to/easylist/easyprivacy.txt",
    ),
    (
        "ublock-filters.txt",
        "https://ublockorigin.github.io/uAssets/filters/filters.txt",
    ),
    MALWARE_FILTER_LIST,
];
const THREAT_WARNING_HTML: &str = r#"<!doctype html><meta charset="utf-8"><title>Unsafe site blocked</title><style>body{margin:0;background:#11131a;color:#e8eaf2;font:16px system-ui;display:grid;min-height:100vh;place-items:center}.card{max-width:560px;padding:32px;border:1px solid #49445c;border-radius:14px;background:#1b1e29}h1{color:#ffb86b}</style><main class="card"><h1>Potentially unsafe site blocked</h1><p>This address appears on the browser's malware blocklist. The page was not loaded.</p><p>Use the Back button to return to safety.</p></main>"#;
const MAX_FILTER_LIST_BYTES: usize = 8 * 1024 * 1024;
const MAX_CUSTOM_FILTER_CACHE_BYTES: usize = 16 * 1024 * 1024;
const FILTER_UPDATE_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const GENERIC_COSMETIC_MESSAGE: &str = "eefi-generic-cosmetic-v1";
const MAX_COSMETIC_MESSAGE_BYTES: usize = 64 * 1024;
const MAX_COSMETIC_BATCH_ITEMS: usize = 128;
const MAX_COSMETIC_TOKEN_LENGTH: usize = 128;
const MAX_COSMETIC_UNIQUE_ITEMS: usize = 2048;
const MAX_COSMETIC_BATCHES: usize = 64;
const MAX_COSMETIC_REPLY_SELECTORS: usize = 128;
const MAX_COSMETIC_SELECTOR_LENGTH: usize = 256;
const MAX_COSMETIC_FRAMES: usize = 256;
const MAX_COSMETIC_WEBVIEW_BATCHES: usize = 512;
const MAX_COSMETIC_WEBVIEW_TOKENS: usize = 8192;
const MAX_PROCEDURAL_COSMETIC_RULES: usize = 128;
const MAX_PROCEDURAL_COSMETIC_SELECTOR_LENGTH: usize = 512;
const MAX_PROCEDURAL_COSMETIC_TEXT_LENGTH: usize = 256;
static SCRIPTLET_RESOURCES: OnceLock<Vec<Resource>> = OnceLock::new();

const SHADOW_DOM_BOOTSTRAP: &str = r#"
(()=>{
const key='__eefiCosmeticRootsV1';
if(window[key])return;
const roots=new Set(),listeners=new Set();
let documentReady=false;
const observer=new MutationObserver(records=>{
 for(const record of records)for(const node of record.addedNodes)discover(node);
 if(!documentReady&&document.documentElement){
  documentReady=true;
  for(const listener of listeners)try{listener(document)}catch(_){}
 }
});
function notify(root){
 for(const listener of listeners)try{listener(root)}catch(_){}
}
function register(root){
 if(!root||roots.has(root))return;
 roots.add(root);
 observer.observe(root,{subtree:true,childList:true});
 notify(root);
 discover(root);
}
function discover(node){
 if(!node||!node.querySelectorAll)return;
 if(node.nodeType===1&&node.shadowRoot)register(node.shadowRoot);
 for(const element of node.querySelectorAll('*')){
  if(element.shadowRoot)register(element.shadowRoot);
 }
}
const manager={
 roots,
 subscribe(listener){
  listeners.add(listener);
  for(const root of roots)notifyRoot(listener,root);
  return ()=>listeners.delete(listener);
 }
};
function notifyRoot(listener,root){try{listener(root)}catch(_){}}
Object.freeze(manager);
Object.defineProperty(window,key,{value:manager,configurable:false,writable:false});
const attachShadow=Element.prototype.attachShadow;
Element.prototype.attachShadow=function(init){
 const root=Reflect.apply(attachShadow,this,arguments);
 if(init&&init.mode==='open')register(root);
 return root;
};
register(document);
})();
"#;

const PROCEDURAL_COSMETIC_BOOTSTRAP: &str = r#"
(()=>{
const rules=__EEFI_RULES__;
const apply=(element,rule)=>{
 if(!element.isConnected)return false;
 for(const text of rule.hasText)if(!element.textContent.includes(text))return;
 const action=rule.action;
 if(!action){element.style.setProperty('display','none','important');return true;}
 if(action.type==='remove'){element.remove();return true;}
 if(action.type==='remove-attr'){
  if(/^[A-Za-z_:][A-Za-z0-9_.:-]*$/.test(action.arg))element.removeAttribute(action.arg);
  return true;
 }
 if(action.type==='remove-class'){
  if(action.arg&&!/[\s\u0000-\u001f]/.test(action.arg))element.classList.remove(action.arg);
  return true;
 }
 if(action.type==='style'){
  const parsed=document.createElement('span').style;
  parsed.cssText=action.arg;
  for(let i=0;i<parsed.length;i++){
   const name=parsed[i];
   element.style.setProperty(name,parsed.getPropertyValue(name),parsed.getPropertyPriority(name));
  }
  return true;
 }
 return false;
};
const roots=new Set(),work=[];
const observed=new WeakSet();
let scheduled=false;
const scheduleFrame=()=>{
 if(scheduled)return;
 scheduled=true;
 requestAnimationFrame(()=>{
  scheduled=false;
  let scanned=0;
  for(const root of roots){
   roots.delete(root);
   scan(root);
   if(++scanned>=32)break;
  }
  let budget=256;
  while(work.length&&budget){
   const task=work[0];
   while(task.index<task.elements.length&&budget){
    apply(task.elements[task.index++],task.rule);
    budget--;
   }
   if(task.index>=task.elements.length)work.shift();
  }
  if(roots.size||work.length)scheduleFrame();
 });
};
const scan=(root)=>{
 if(!root||!root.querySelectorAll)return;
 for(const rule of rules){
  try{
   if(root.nodeType===1&&root.matches(rule.selector))work.push({rule,elements:[root],index:0});
   const elements=root.querySelectorAll(rule.selector);
   if(elements.length)work.push({rule,elements,index:0});
  }catch(_){}
 }
};
const schedule=(node)=>{
 const root=node&&node.nodeType===1?node:node&&node.parentElement;
 if(!root)return;
 roots.add(root);
 if(roots.size>256){
  roots.clear();
  if(document.documentElement)roots.add(document.documentElement);
 }
 scheduleFrame();
};
const observeRoot=(root)=>{
 if(observed.has(root))return;
 observed.add(root);
 schedule(root);
 new MutationObserver(records=>{
  for(const record of records){
   if(record.type==='childList'){
    for(const node of record.addedNodes)schedule(node);
   }else schedule(record.target);
  }
 }).observe(root,{subtree:true,childList:true,characterData:true});
};
window.__eefiCosmeticRootsV1.subscribe(observeRoot);
})();
"#;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProceduralCosmeticRule {
    selector: String,
    has_text: Vec<String>,
    action: Option<CosmeticFilterAction>,
}

const GENERIC_COSMETIC_BOOTSTRAP: &str = r#"
(()=>{
const w=window.chrome&&window.chrome.webview;
if(w){
 const classesSeen=new Set(),idsSeen=new Set(),applied=new Set();
 const appliedRules=[],styles=new WeakMap(),observed=new WeakSet();
 let pendingClasses=[],pendingIds=[],messagesSent=0;
 const addToken=(token,set,pending)=>{
  if(typeof token!=='string'||!token||token.length>128||/[\s\u0000-\u001f]/.test(token)||
     set.size>=2048||set.has(token))return;
  set.add(token);pending.push(token);
 };
 const collectElement=(element)=>{
  if(!element||element.nodeType!==1)return;
  for(const name of element.classList)addToken(name,classesSeen,pendingClasses);
  if(element.id)addToken(element.id,idsSeen,pendingIds);
 };
 const collectRoot=(root)=>{
  if(root.nodeType===1)collectElement(root);
  if(root.querySelectorAll){
   for(const element of root.querySelectorAll('[class],[id]'))collectElement(element);
  }
 };
 const installStyle=(root)=>{
  if(!appliedRules.length)return;
  const target=root.nodeType===9?root.documentElement:root;
  if(!target)return;
  let style=styles.get(root);
  if(!style){
   style=document.createElement('style');
   style.dataset.eefiGenericCosmetic='';
   target.appendChild(style);
   styles.set(root,style);
  }
  style.textContent=appliedRules.join('\n')+'\n';
 };
 const flush=()=>{
  while((pendingClasses.length||pendingIds.length)&&messagesSent<64){
   w.postMessage({kind:'eefi-generic-cosmetic-v1',
    classes:pendingClasses.splice(0,64),ids:pendingIds.splice(0,64)});
   messagesSent++;
  }
 };
 const response=(event)=>{
  const data=event.data;
  if(!data||data.kind!=='eefi-generic-cosmetic-v1'||!Array.isArray(data.selectors))return;
  const rules=[];let received=0;
  for(const selector of data.selectors){
   if(received>=128)break;
   received++;
   if(typeof selector!=='string'||!selector||selector.length>256||applied.has(selector))continue;
   applied.add(selector);rules.push(selector+'{display:none!important}');
  }
  if(!rules.length)return;
  appliedRules.push(...rules);
  for(const root of window.__eefiCosmeticRootsV1.roots)installStyle(root);
 };
 w.addEventListener('message',response);
 const observeRoot=(root)=>{
  if(observed.has(root))return;
  observed.add(root);
  collectRoot(root);
  new MutationObserver(records=>{
   for(const record of records){
    if(record.type==='attributes')collectElement(record.target);
    else for(const node of record.addedNodes)collectRoot(node);
   }
   flush();
  }).observe(root,{subtree:true,childList:true,attributes:true,attributeFilter:['class','id']});
  installStyle(root);
  flush();
 };
 window.__eefiCosmeticRootsV1.subscribe(observeRoot);
}
})();
"#;

#[derive(Clone)]
pub struct AdblockService(Arc<AdblockInner>);

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomFilterConfig {
    list_urls: Vec<String>,
    user_rules: String,
}

struct AdblockInner {
    engine: RwLock<Engine>,
    enabled: AtomicBool,
    allowed_sites: RwLock<std::collections::HashSet<String>>,
    blocked_counts: Mutex<HashMap<String, u64>>,
    malicious_domains: RwLock<HashSet<String>>,
    custom_filter_config: RwLock<CustomFilterConfig>,
    filter_cache_directory: RwLock<Option<PathBuf>>,
    filter_refresh_lock: tokio::sync::Mutex<()>,
    preference_lock: Mutex<()>,
    global_preference_changed: AtomicBool,
    modified_sites: Mutex<std::collections::HashSet<String>>,
    preferences_loaded: AtomicBool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdblockStatus {
    enabled: bool,
    site_allowed: bool,
    blocked_count: u64,
}

// Async registrations can complete after later navigations, so stale results must be discarded.
#[derive(Default)]
struct CosmeticScriptRegistry {
    epoch: u64,
    next_generation: u64,
    generations: HashMap<String, u64>,
    script_ids: HashMap<String, String>,
    generic_quotas: HashMap<String, GenericCosmeticQuota>,
    generic_batches_total: usize,
    generic_tokens_total: usize,
}

#[derive(Default)]
struct GenericCosmeticQuota {
    page_key: Option<String>,
    batches: usize,
    seen_classes: HashSet<String>,
    seen_ids: HashSet<String>,
    invalid_messages_logged: u8,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GenericCosmeticRequest {
    kind: String,
    classes: Vec<String>,
    ids: Vec<String>,
}

#[derive(Serialize)]
struct GenericCosmeticReply {
    kind: &'static str,
    selectors: Vec<String>,
}

type CosmeticScriptRegistryRef = std::rc::Rc<std::cell::RefCell<CosmeticScriptRegistry>>;

impl AdblockService {
    pub fn new() -> Self {
        Self(Arc::new(AdblockInner {
            engine: RwLock::new(build_engine(BUILTIN_FILTERS)),
            enabled: AtomicBool::new(true),
            allowed_sites: RwLock::new(std::collections::HashSet::new()),
            blocked_counts: Mutex::new(HashMap::new()),
            malicious_domains: RwLock::new(HashSet::new()),
            custom_filter_config: RwLock::new(CustomFilterConfig::default()),
            filter_cache_directory: RwLock::new(None),
            filter_refresh_lock: tokio::sync::Mutex::new(()),
            preference_lock: Mutex::new(()),
            global_preference_changed: AtomicBool::new(false),
            modified_sites: Mutex::new(std::collections::HashSet::new()),
            preferences_loaded: AtomicBool::new(false),
        }))
    }

    fn should_block(
        &self,
        request_url: &str,
        source_url: &str,
        request_type: &str,
        method: &str,
    ) -> Result<bool, String> {
        if !self.0.enabled.load(Ordering::Relaxed) {
            return Ok(false);
        }

        let parsed_source = Url::parse(source_url).ok();
        let source_host = parsed_source
            .as_ref()
            .and_then(Url::host_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !source_host.is_empty()
            && self
                .0
                .allowed_sites
                .read()
                .map_err(|_| "ad blocker allowlist lock is poisoned".to_string())?
                .contains(&source_host)
        {
            return Ok(false);
        }

        let match_source = if source_host.is_empty() {
            request_url
        } else {
            source_url
        };
        let request = match Request::new(request_url, match_source, request_type, method) {
            Ok(request) => request,
            Err(_) => return Ok(false),
        };
        let blocked = self
            .0
            .engine
            .read()
            .map_err(|_| "ad blocker engine lock is poisoned".to_string())?
            .check_network_request(&request)
            .should_block();

        if blocked && !source_host.is_empty() {
            let mut counts = self
                .0
                .blocked_counts
                .lock()
                .map_err(|_| "ad blocker counter lock is poisoned".to_string())?;
            let count = counts.entry(source_host).or_default();
            *count = count.saturating_add(1);
        }
        Ok(blocked)
    }

    fn status(&self, host: &str) -> Result<AdblockStatus, String> {
        let host = if host.trim().is_empty() {
            None
        } else {
            Some(normalize_host(host)?)
        };
        let site_allowed = match &host {
            Some(host) => self
                .0
                .allowed_sites
                .read()
                .map_err(|_| "ad blocker allowlist lock is poisoned".to_string())?
                .contains(host),
            None => false,
        };
        let blocked_count = match &host {
            Some(host) => self
                .0
                .blocked_counts
                .lock()
                .map_err(|_| "ad blocker counter lock is poisoned".to_string())?
                .get(host)
                .copied()
                .unwrap_or_default(),
            None => 0,
        };
        Ok(AdblockStatus {
            enabled: self.0.enabled.load(Ordering::Relaxed),
            site_allowed,
            blocked_count,
        })
    }

    fn is_malicious_url(&self, value: &str) -> Result<bool, String> {
        let Some(host) = Url::parse(value)
            .ok()
            .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        else {
            return Ok(false);
        };
        let domains = self
            .0
            .malicious_domains
            .read()
            .map_err(|_| "malware domain list lock is poisoned".to_string())?;
        let mut candidate = host.as_str();
        loop {
            if domains.contains(candidate) {
                return Ok(true);
            }
            let Some(separator) = candidate.find('.') else {
                return Ok(false);
            };
            candidate = &candidate[separator + 1..];
        }
    }

    fn replace_malicious_domains(&self, domains: HashSet<String>) -> Result<(), String> {
        *self
            .0
            .malicious_domains
            .write()
            .map_err(|_| "malware domain list lock is poisoned".to_string())? = domains;
        Ok(())
    }

    fn custom_filter_config(&self) -> Result<CustomFilterConfig, String> {
        self.0
            .custom_filter_config
            .read()
            .map(|config| config.clone())
            .map_err(|_| "custom filter settings lock is poisoned".to_string())
    }

    fn set_custom_filter_config(&self, config: CustomFilterConfig) -> Result<(), String> {
        *self
            .0
            .custom_filter_config
            .write()
            .map_err(|_| "custom filter settings lock is poisoned".to_string())? = config;
        Ok(())
    }

    fn set_filter_cache_directory(&self, directory: PathBuf) -> Result<(), String> {
        *self
            .0
            .filter_cache_directory
            .write()
            .map_err(|_| "filter cache directory lock is poisoned".to_string())? = Some(directory);
        Ok(())
    }

    fn filter_cache_directory(&self) -> Result<PathBuf, String> {
        self.0
            .filter_cache_directory
            .read()
            .map_err(|_| "filter cache directory lock is poisoned".to_string())?
            .clone()
            .ok_or_else(|| "ad blocker filter cache is not initialized".to_string())
    }

    fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        let _guard = self
            .0
            .preference_lock
            .lock()
            .map_err(|_| "ad blocker preference lock is poisoned".to_string())?;
        self.0.enabled.store(enabled, Ordering::Relaxed);
        self.0
            .global_preference_changed
            .store(true, Ordering::Relaxed);
        Ok(())
    }

    fn set_site_allowed(&self, host: &str, allowed: bool) -> Result<(), String> {
        let _guard = self
            .0
            .preference_lock
            .lock()
            .map_err(|_| "ad blocker preference lock is poisoned".to_string())?;
        self.0
            .modified_sites
            .lock()
            .map_err(|_| "ad blocker preference lock is poisoned".to_string())?
            .insert(host.to_string());
        let mut allowed_sites = self
            .0
            .allowed_sites
            .write()
            .map_err(|_| "ad blocker allowlist lock is poisoned".to_string())?;
        if allowed {
            allowed_sites.insert(host.to_string());
        } else {
            allowed_sites.remove(host);
        }
        Ok(())
    }

    fn restore_preferences(
        &self,
        enabled: bool,
        allowed_sites: std::collections::HashSet<String>,
        custom_filter_config: CustomFilterConfig,
    ) -> Result<(), String> {
        let _guard = self
            .0
            .preference_lock
            .lock()
            .map_err(|_| "ad blocker preference lock is poisoned".to_string())?;
        if !self.0.global_preference_changed.load(Ordering::Relaxed) {
            self.0.enabled.store(enabled, Ordering::Relaxed);
        }
        let modified_sites = self
            .0
            .modified_sites
            .lock()
            .map_err(|_| "ad blocker preference lock is poisoned".to_string())?;
        let mut current_sites = self
            .0
            .allowed_sites
            .write()
            .map_err(|_| "ad blocker allowlist lock is poisoned".to_string())?;
        current_sites.extend(
            allowed_sites
                .into_iter()
                .filter(|host| !modified_sites.contains(host)),
        );
        *self
            .0
            .custom_filter_config
            .write()
            .map_err(|_| "custom filter settings lock is poisoned".to_string())? =
            custom_filter_config;
        self.0.preferences_loaded.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn replace_engine(&self, engine: Engine) -> Result<(), String> {
        *self
            .0
            .engine
            .write()
            .map_err(|_| "ad blocker engine lock is poisoned".to_string())? = engine;
        Ok(())
    }

    fn cosmetic_script(
        &self,
        page_url: &str,
        allow_generic: bool,
    ) -> Result<Option<String>, String> {
        if !self.0.enabled.load(Ordering::Relaxed) {
            return Ok(None);
        }

        let page = Url::parse(page_url).map_err(|error| format!("invalid page URL: {error}"))?;
        if !matches!(page.scheme(), "http" | "https") {
            return Ok(None);
        }

        let host = page.host_str().unwrap_or_default().to_ascii_lowercase();
        if self
            .0
            .allowed_sites
            .read()
            .map_err(|_| "ad blocker allowlist lock is poisoned".to_string())?
            .contains(&host)
        {
            return Ok(None);
        }

        let resources = self
            .0
            .engine
            .read()
            .map_err(|_| "ad blocker engine lock is poisoned".to_string())?
            .url_cosmetic_resources(page_url);
        let mut procedural_candidates =
            resources.procedural_actions.into_iter().collect::<Vec<_>>();
        procedural_candidates.sort_unstable();
        let procedural_actions = procedural_candidates
            .into_iter()
            .take(MAX_PROCEDURAL_COSMETIC_RULES * 4)
            .map(|rule| {
                serde_json::from_str::<ProceduralOrActionFilter>(&rule)
                    .map_err(|error| format!("invalid procedural cosmetic rule: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter_map(supported_procedural_action)
            .take(MAX_PROCEDURAL_COSMETIC_RULES)
            .collect::<Vec<_>>();
        if resources.hide_selectors.is_empty()
            && resources.injected_script.is_empty()
            && procedural_actions.is_empty()
            && (resources.generichide || !allow_generic)
        {
            return Ok(None);
        }

        let target_origin = page.origin().ascii_serialization();
        let target_origin = serde_json::to_string(&target_origin)
            .map_err(|error| format!("could not encode cosmetic target origin: {error}"))?;
        let target_path = serde_json::to_string(page.path())
            .map_err(|error| format!("could not encode cosmetic target path: {error}"))?;
        let target_query = page
            .query()
            .map(|query| format!("?{query}"))
            .unwrap_or_default();
        let target_query = serde_json::to_string(&target_query)
            .map_err(|error| format!("could not encode cosmetic target query: {error}"))?;

        let mut script = format!(
            "(()=>{{const t={{origin:{target_origin},pathname:{target_path},search:{target_query}}};\
             const u=new URL(location.href);\
             if(u.origin!==t.origin||u.pathname!==t.pathname||u.search!==t.search)return;"
        );
        script.push_str(SHADOW_DOM_BOOTSTRAP);

        if !resources.hide_selectors.is_empty() {
            let css = resources
                .hide_selectors
                .into_iter()
                .map(|selector| format!("{selector} {{ display: none !important; }}"))
                .collect::<Vec<_>>()
                .join("\n");
            let css = serde_json::to_string(&css)
                .map_err(|error| format!("could not encode cosmetic CSS: {error}"))?;
            script.push_str(&format!(
                "(()=>{{const css={css};const styles=new WeakMap(),observed=new WeakSet();\
                 const install=root=>{{const target=root.nodeType===9?root.documentElement:root;\
                 if(!target)return;let style=styles.get(root);if(!style){{\
                 style=document.createElement('style');style.dataset.eefiCosmetic='';\
                 styles.set(root,style);}}if(style.parentNode!==target)\
                 target.insertBefore(style,target.firstChild);\
                 if(style.textContent!==css)style.textContent=css;}};\
                 const observe=root=>{{install(root);if(observed.has(root))return;\
                 observed.add(root);new MutationObserver(()=>install(root))\
                 .observe(root,{{childList:true,subtree:true}});}};\
                 window.__eefiCosmeticRootsV1.subscribe(observe);}})();"
            ));
        }
        if !resources.injected_script.is_empty() {
            script.push_str("\n(()=>{");
            script.push_str(&resources.injected_script);
            script.push_str("})();");
        }
        if !procedural_actions.is_empty() {
            let rules = serde_json::to_string(&procedural_actions)
                .map_err(|error| format!("could not encode procedural cosmetic rules: {error}"))?
                .replace('\u{2028}', "\\u2028")
                .replace('\u{2029}', "\\u2029");
            script.push_str(&PROCEDURAL_COSMETIC_BOOTSTRAP.replace("__EEFI_RULES__", &rules));
        }
        if !resources.generichide && allow_generic {
            script.push_str(GENERIC_COSMETIC_BOOTSTRAP);
        }
        script.push_str("})();");

        Ok(Some(script))
    }
}

fn supported_procedural_action(rule: ProceduralOrActionFilter) -> Option<ProceduralCosmeticRule> {
    let mut selector = None;
    let mut has_text = Vec::new();
    for operator in &rule.selector {
        match operator {
            CosmeticFilterOperator::CssSelector(value) if selector.is_none() => {
                let (base_selector, embedded_text) = split_text_pseudo(value)?;
                selector = Some(base_selector);
                has_text.extend(embedded_text);
            }
            CosmeticFilterOperator::HasText(text) if is_supported_text_match(text) => {
                has_text.push(text.clone());
            }
            _ => return None,
        }
    }
    let selector = selector?;
    if selector.is_empty()
        || selector.len() > MAX_PROCEDURAL_COSMETIC_SELECTOR_LENGTH
        || has_text.len() > 2
        || has_text.iter().any(|text| !is_supported_text_match(text))
    {
        return None;
    }
    let action_is_supported = match &rule.action {
        None | Some(CosmeticFilterAction::Remove) => true,
        Some(CosmeticFilterAction::Style(style)) => style.len() <= 512,
        Some(CosmeticFilterAction::RemoveAttr(attribute)) => {
            !attribute.is_empty()
                && attribute.len() <= 128
                && attribute.chars().enumerate().all(|(index, character)| {
                    if index == 0 {
                        character.is_ascii_alphabetic() || matches!(character, '_' | ':')
                    } else {
                        character.is_ascii_alphanumeric()
                            || matches!(character, '_' | ':' | '.' | '-')
                    }
                })
        }
        Some(CosmeticFilterAction::RemoveClass(class)) => {
            !class.is_empty()
                && class.len() <= 128
                && !class
                    .chars()
                    .any(|character| character.is_whitespace() || character.is_control())
        }
    };
    action_is_supported.then_some(ProceduralCosmeticRule {
        selector,
        has_text,
        action: rule.action,
    })
}

fn split_text_pseudo(selector: &str) -> Option<(String, Vec<String>)> {
    let mut remaining = selector;
    let mut base_selector = None;
    let mut text_matches = Vec::new();
    loop {
        let Some(start) = remaining.find(":has-text(") else {
            if base_selector.is_some() && !remaining.is_empty() {
                return None;
            }
            if base_selector.is_none() {
                return Some((selector.to_string(), text_matches));
            }
            break;
        };
        if base_selector.is_none() {
            base_selector = Some(remaining[..start].to_string());
        } else if start != 0 {
            return None;
        }
        let argument = &remaining[start + ":has-text(".len()..];
        let end = argument.find(')')?;
        let text = &argument[..end];
        if text.contains('(') || !is_supported_text_match(text) {
            return None;
        }
        text_matches.push(text.to_string());
        remaining = &argument[end + 1..];
        if remaining.is_empty() {
            break;
        }
        if !remaining.starts_with(":has-text(") {
            return None;
        }
    }
    Some((base_selector?, text_matches))
}

fn is_supported_text_match(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_PROCEDURAL_COSMETIC_TEXT_LENGTH
        && !text.starts_with('/')
        && !text
            .chars()
            .any(|character| matches!(character, '\'' | '"' | '\\') || character.is_control())
}

fn build_engine(filter_text: &str) -> Engine {
    let mut filters = FilterSet::new(false);
    filters.add_filter_list(filter_text.to_string(), ParseOptions::default());
    #[cfg(feature = "live-verification")]
    filters.add_filter_list(
        "localhost##.eefi-shadow-ad\n\
         localhost##.eefi-generic-ad\n\
         localhost##+js(set-constant, __eefiScriptletProbe, true)\n\
         ||localhost*blocked-fetch.txt^$xmlhttprequest\n\
         ||localhost*blocked-image.svg^$image\n\
         ||localhost*blocked-document.html^$document"
            .to_string(),
        ParseOptions::default(),
    );
    let mut engine = Engine::new_with_filter_set(filters);
    install_scriptlet_resources(&mut engine);
    engine
}

fn install_scriptlet_resources(engine: &mut Engine) {
    let resources = SCRIPTLET_RESOURCES.get_or_init(|| {
        serde_json::from_slice(include_bytes!(concat!(
            env!("OUT_DIR"),
            "\\ublock-scriptlet-resources.json"
        )))
        .expect("generated uBlock scriptlet resource data is valid")
    });
    engine.use_resources(resources.iter().cloned());
}

#[cfg(all(test, feature = "live-verification"))]
#[test]
fn live_verification_rules_are_loopback_scoped() {
    let engine = build_engine("");
    let local_url = "http://localhost:1420/__stage3__/fixture.html";
    let local = engine.url_cosmetic_resources(local_url);
    assert!(local.hide_selectors.contains(".eefi-shadow-ad"));
    assert!(local.hide_selectors.contains(".eefi-generic-ad"));
    assert!(local.injected_script.contains("__eefiScriptletProbe"));

    let remote = engine.url_cosmetic_resources("https://example.com/");
    assert!(!remote.hide_selectors.contains(".eefi-shadow-ad"));
    assert!(!remote.hide_selectors.contains(".eefi-generic-ad"));
    assert!(!remote.injected_script.contains("__eefiScriptletProbe"));

    let service = AdblockService::new();
    let injected = service
        .cosmetic_script(local_url, true)
        .expect("build loopback cosmetic script")
        .expect("loopback cosmetics should be present");
    assert!(injected.contains(".eefi-shadow-ad { display: none !important; }"));
    assert!(injected.contains("window.__eefiCosmeticRootsV1.subscribe"));
    assert!(injected.contains("__eefiScriptletProbe"));

    let local_request = Request::new(
        "http://localhost:1420/__stage3__/blocked-fetch.txt",
        "http://localhost:1420/__stage3__/fixture.html",
        "xmlhttprequest",
        "GET",
    )
    .expect("construct local verification request");
    assert!(engine.check_network_request(&local_request).should_block());

    let remote_request = Request::new(
        "https://example.com/__stage3__/blocked-fetch.txt",
        "https://example.com/",
        "xmlhttprequest",
        "GET",
    )
    .expect("construct remote request");
    assert!(!engine.check_network_request(&remote_request).should_block());

    let local_image = Request::new(
        "http://localhost:1420/__stage3__/blocked-image.svg",
        "http://localhost:1420/__stage3__/fixture.html",
        "image",
        "GET",
    )
    .expect("construct local image request");
    assert!(engine.check_network_request(&local_image).should_block());

    let local_document = Request::new(
        "http://localhost:1420/__stage3__/blocked-document.html",
        "http://localhost:1420/__stage3__/fixture.html",
        "document",
        "GET",
    )
    .expect("construct local document request");
    assert!(engine.check_network_request(&local_document).should_block());
}

fn cosmetic_page_key(page_url: &str) -> Option<String> {
    let mut page = Url::parse(page_url).ok()?;
    if !matches!(page.scheme(), "http" | "https") {
        return None;
    }
    page.set_fragment(None);
    Some(page.to_string())
}

fn is_generic_cosmetic_message(message_json: &str) -> bool {
    if message_json.len() > MAX_COSMETIC_MESSAGE_BYTES {
        return false;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(message_json) else {
        return false;
    };
    value.get("kind").and_then(serde_json::Value::as_str) == Some(GENERIC_COSMETIC_MESSAGE)
}

fn process_generic_cosmetic_message(
    service: &AdblockService,
    registry: &CosmeticScriptRegistryRef,
    frame_key: &str,
    source_url: &str,
    message_json: &str,
) -> Result<Option<String>, String> {
    if message_json.len() > MAX_COSMETIC_MESSAGE_BYTES {
        return Err("cosmetic message exceeded the size limit".to_string());
    }
    let request: GenericCosmeticRequest = serde_json::from_str(message_json)
        .map_err(|error| format!("invalid cosmetic message: {error}"))?;
    if request.kind != GENERIC_COSMETIC_MESSAGE {
        return Err("unsupported cosmetic message type".to_string());
    }
    if request.classes.len() + request.ids.len() > MAX_COSMETIC_BATCH_ITEMS {
        return Err("cosmetic message contained too many tokens".to_string());
    }
    if request.classes.iter().chain(&request.ids).any(|token| {
        token.is_empty()
            || token.len() > MAX_COSMETIC_TOKEN_LENGTH
            || token
                .chars()
                .any(|character| character.is_whitespace() || character.is_control())
    }) {
        return Err("cosmetic message contained an invalid token".to_string());
    }

    let page_key = cosmetic_page_key(source_url)
        .ok_or_else(|| "invalid cosmetic message source".to_string())?;
    let (classes, ids) = {
        let mut registry = registry.borrow_mut();
        if registry.generic_batches_total >= MAX_COSMETIC_WEBVIEW_BATCHES {
            return Ok(None);
        }
        let initial_remaining =
            MAX_COSMETIC_WEBVIEW_TOKENS.saturating_sub(registry.generic_tokens_total);
        let mut remaining = initial_remaining;
        let Some(quota) = registry.generic_quotas.get_mut(frame_key) else {
            return Ok(None);
        };
        if quota.page_key.as_deref() != Some(&page_key) || quota.batches >= MAX_COSMETIC_BATCHES {
            return Ok(None);
        }
        quota.batches += 1;
        let classes = request
            .classes
            .into_iter()
            .filter(|class| {
                if quota.seen_classes.len() >= MAX_COSMETIC_UNIQUE_ITEMS || remaining == 0 {
                    return false;
                }
                if quota.seen_classes.insert(class.clone()) {
                    remaining -= 1;
                    true
                } else {
                    false
                }
            })
            .collect::<Vec<_>>();
        let ids = request
            .ids
            .into_iter()
            .filter(|id| {
                if quota.seen_ids.len() >= MAX_COSMETIC_UNIQUE_ITEMS || remaining == 0 {
                    return false;
                }
                if quota.seen_ids.insert(id.clone()) {
                    remaining -= 1;
                    true
                } else {
                    false
                }
            })
            .collect::<Vec<_>>();
        registry.generic_batches_total += 1;
        registry.generic_tokens_total += initial_remaining - remaining;
        (classes, ids)
    };
    if classes.is_empty() && ids.is_empty() {
        return Ok(None);
    }
    if !service.0.enabled.load(Ordering::Relaxed) {
        return Ok(None);
    }

    let page = Url::parse(source_url).map_err(|error| format!("invalid page URL: {error}"))?;
    let host = page.host_str().unwrap_or_default().to_ascii_lowercase();
    if service
        .0
        .allowed_sites
        .read()
        .map_err(|_| "ad blocker allowlist lock is poisoned".to_string())?
        .contains(&host)
    {
        return Ok(None);
    }

    let engine = service
        .0
        .engine
        .read()
        .map_err(|_| "ad blocker engine lock is poisoned".to_string())?;
    let resources = engine.url_cosmetic_resources(source_url);
    if resources.generichide {
        return Ok(None);
    }
    let selectors = engine
        .hidden_class_id_selectors(classes, ids, &resources.exceptions)
        .into_iter()
        .filter(|selector| !selector.is_empty() && selector.len() <= MAX_COSMETIC_SELECTOR_LENGTH)
        .take(MAX_COSMETIC_REPLY_SELECTORS)
        .collect::<Vec<_>>();
    if selectors.is_empty() {
        return Ok(None);
    }

    serde_json::to_string(&GenericCosmeticReply {
        kind: GENERIC_COSMETIC_MESSAGE,
        selectors,
    })
    .map(Some)
    .map_err(|error| format!("could not encode generic cosmetic reply: {error}"))
}

fn log_cosmetic_message_error(registry: &CosmeticScriptRegistryRef, frame_key: &str, error: &str) {
    let should_log = {
        let mut registry = registry.borrow_mut();
        registry
            .generic_quotas
            .get_mut(frame_key)
            .map(|quota| {
                if quota.invalid_messages_logged < 3 {
                    quota.invalid_messages_logged += 1;
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false)
    };
    if should_log {
        eprintln!("ignored invalid generic cosmetic message: {error}");
    }
}

pub fn start_filter_updates(service: AdblockService, directory: PathBuf, storage: StorageService) {
    if let Err(error) = service.set_filter_cache_directory(directory.clone()) {
        eprintln!("could not configure ad blocker cache directory: {error}");
        return;
    }
    tauri::async_runtime::spawn(async move {
        if let Err(error) = ensure_preferences_loaded(&storage, &service).await {
            eprintln!("could not load ad blocker preferences: {error}");
        }

        let custom_filter_config = match service.custom_filter_config() {
            Ok(config) => config,
            Err(error) => {
                eprintln!("could not read custom filter configuration: {error}");
                return;
            }
        };
        let _cache_guard = service.0.filter_refresh_lock.lock().await;
        let load_directory = directory.clone();
        match tauri::async_runtime::spawn_blocking(move || {
            load_cached_engine(&load_directory, &custom_filter_config)
        })
        .await
        {
            Ok(Ok(engine)) => {
                if let Err(error) = service.replace_engine(engine) {
                    eprintln!("could not install cached ad blocker engine: {error}");
                }
            }
            Ok(Err(error)) => eprintln!("could not load ad blocker cache: {error}"),
            Err(error) => eprintln!("ad blocker cache worker failed: {error}"),
        }
        let domains_directory = directory.clone();
        match tauri::async_runtime::spawn_blocking(move || {
            read_cached_filter_lists(&domains_directory)
                .map(|lists| extract_malicious_domains(&lists))
        })
        .await
        {
            Ok(Ok(domains)) => {
                if let Err(error) = service.replace_malicious_domains(domains) {
                    eprintln!("could not install cached malware domains: {error}");
                }
            }
            Ok(Err(error)) => eprintln!("could not load cached malware domains: {error}"),
            Err(error) => eprintln!("malware-list cache worker failed: {error}"),
        }
        drop(_cache_guard);

        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
        {
            Ok(client) => client,
            Err(error) => {
                eprintln!("could not create ad blocker update client: {error}");
                return;
            }
        };

        loop {
            let check_directory = directory.clone();
            let stale = match tauri::async_runtime::spawn_blocking(move || {
                filter_cache_is_stale(&check_directory)
            })
            .await
            {
                Ok(Ok(stale)) => stale,
                Ok(Err(error)) => {
                    eprintln!("could not check ad blocker filter age: {error}");
                    true
                }
                Err(error) => {
                    eprintln!("ad blocker cache worker failed: {error}");
                    true
                }
            };

            if stale {
                if let Err(error) = refresh_filter_lists(&client, &service, &directory).await {
                    eprintln!("could not update ad blocker filter lists: {error}");
                }
            }
            tokio::time::sleep(FILTER_UPDATE_INTERVAL).await;
        }
    });
}

pub async fn ensure_preferences_loaded(
    storage: &StorageService,
    service: &AdblockService,
) -> Result<(), String> {
    if service.0.preferences_loaded.load(Ordering::Relaxed) {
        return Ok(());
    }
    let (enabled, allowed_sites, custom_filter_config) = storage
        .with_connection(|connection| load_preferences(connection))
        .await?;
    service.restore_preferences(enabled, allowed_sites, custom_filter_config)
}

async fn refresh_filter_lists(
    client: &reqwest::Client,
    service: &AdblockService,
    directory: &Path,
) -> Result<(), String> {
    let _refresh_guard = service.0.filter_refresh_lock.lock().await;
    let custom_filter_config = service.custom_filter_config()?;
    let read_directory = directory.to_path_buf();
    let (mut cached, mut custom_cached) = tauri::async_runtime::spawn_blocking(move || {
        Ok::<_, String>((
            read_cached_filter_lists(&read_directory)?,
            read_custom_filter_cache(&read_directory)?,
        ))
    })
    .await
    .map_err(|error| format!("filter cache worker failed: {error}"))??;
    for (filename, url) in FILTER_LISTS {
        match fetch_filter_list(client, url).await {
            Ok(contents) => {
                cached.insert(filename.to_string(), contents);
            }
            Err(error) => eprintln!("filter list update failed for {url}: {error}"),
        }
    }
    custom_cached.retain(|url, _| custom_filter_config.list_urls.contains(url));
    let mut custom_update_failures = Vec::new();
    for (index, url) in custom_filter_config.list_urls.iter().enumerate() {
        match fetch_filter_list(client, url).await {
            Ok(contents) => {
                let updated_size = custom_cached
                    .iter()
                    .filter(|(cached_url, _)| *cached_url != url)
                    .map(|(_, contents)| contents.len())
                    .sum::<usize>()
                    .saturating_add(contents.len());
                if updated_size > MAX_CUSTOM_FILTER_CACHE_BYTES {
                    eprintln!("custom filter cache limit exceeded; keeping previous cache");
                } else {
                    custom_cached.insert(url.clone(), contents);
                }
            }
            Err(error) => {
                eprintln!("custom filter list {} update failed: {error}", index + 1);
                custom_update_failures.push(url.clone());
            }
        }
    }

    let malicious_domains = extract_malicious_domains(&cached);
    let update_directory = directory.to_path_buf();
    let config_for_worker = custom_filter_config.clone();
    let engine = tauri::async_runtime::spawn_blocking(move || {
        persist_filter_lists_and_build_engine(
            &update_directory,
            &cached,
            &custom_cached,
            &config_for_worker,
        )
    })
    .await
    .map_err(|error| format!("filter compiler worker failed: {error}"))??;
    service.replace_engine(engine)?;
    service.replace_malicious_domains(malicious_domains)?;
    if !custom_update_failures.is_empty() {
        return Err(format!(
            "could not refresh {} custom filter list(s); cached lists, when available, remain active",
            custom_update_failures.len()
        ));
    }
    Ok(())
}

async fn fetch_filter_list(client: &reqwest::Client, url: &str) -> Result<String, String> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("request failed: {}", error.without_url()))?
        .error_for_status()
        .map_err(|error| format!("server returned an error: {}", error.without_url()))?;
    if response
        .content_length()
        .map(|length| length > MAX_FILTER_LIST_BYTES as u64)
        .unwrap_or(false)
    {
        return Err("filter list exceeds the configured size limit".to_string());
    }

    let mut stream = response.bytes_stream();
    let mut contents = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(|error| format!("could not read response body: {}", error.without_url()))?;
        if contents.len().saturating_add(chunk.len()) > MAX_FILTER_LIST_BYTES {
            return Err("filter list exceeds the configured size limit".to_string());
        }
        contents.extend_from_slice(&chunk);
    }
    if contents.is_empty() {
        return Err("filter list response was empty".to_string());
    }
    String::from_utf8(contents).map_err(|error| format!("filter list is not UTF-8: {error}"))
}

fn load_cached_engine(
    directory: &Path,
    custom_filter_config: &CustomFilterConfig,
) -> Result<Engine, String> {
    if !cfg!(feature = "live-verification")
        && compiled_engine_is_current(directory, custom_filter_config)
    {
        let bytes = std::fs::read(directory.join("engine.bin"))
            .map_err(|error| format!("could not read compiled ad blocker engine: {error}"))?;
        let mut engine = Engine::default();
        if engine.deserialize(&bytes).is_ok() {
            install_scriptlet_resources(&mut engine);
            return Ok(engine);
        }
    }
    let lists = read_cached_filter_lists(directory)?;
    let mut custom_lists = read_custom_filter_cache(directory)?;
    custom_lists.retain(|url, _| custom_filter_config.list_urls.contains(url));
    let engine = build_engine(&combine_filter_lists(
        &lists,
        &custom_lists,
        custom_filter_config,
    ));
    persist_custom_filter_cache(directory, &custom_lists, custom_filter_config)?;
    persist_engine(directory, &engine)?;
    Ok(engine)
}

fn compiled_engine_is_current(directory: &Path, config: &CustomFilterConfig) -> bool {
    let compiled = match std::fs::metadata(directory.join("engine.bin"))
        .and_then(|metadata| metadata.modified())
    {
        Ok(modified) => modified,
        Err(_) => return false,
    };
    match std::fs::read(directory.join("custom-filter-config.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<CustomFilterConfig>(&bytes).ok())
    {
        Some(persisted) if &persisted == config => {}
        _ => return false,
    }
    for (filename, _) in FILTER_LISTS {
        let list = match std::fs::metadata(directory.join(filename))
            .and_then(|metadata| metadata.modified())
        {
            Ok(modified) => modified,
            Err(_) => return false,
        };
        if list > compiled {
            return false;
        }
    }
    for path in [
        directory.join("custom-filter-lists.json"),
        directory.join("custom-filter-config.json"),
    ] {
        let modified = match std::fs::metadata(path).and_then(|metadata| metadata.modified()) {
            Ok(modified) => modified,
            Err(_) => return false,
        };
        if modified > compiled {
            return false;
        }
    }
    true
}

fn read_cached_filter_lists(directory: &Path) -> Result<HashMap<String, String>, String> {
    let mut lists = HashMap::new();
    for (filename, _) in FILTER_LISTS {
        match std::fs::read_to_string(directory.join(filename)) {
            Ok(contents) if contents.len() <= MAX_FILTER_LIST_BYTES => {
                lists.insert(filename.to_string(), contents);
            }
            Ok(_) => eprintln!("ignoring oversized cached filter list {filename}"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("could not read {filename}: {error}")),
        }
    }
    Ok(lists)
}

fn read_custom_filter_cache(directory: &Path) -> Result<HashMap<String, String>, String> {
    let path = directory.join("custom-filter-lists.json");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(error) => return Err(format!("could not read custom filter cache: {error}")),
    };
    if bytes.len() > MAX_CUSTOM_FILTER_CACHE_BYTES {
        return Err("custom filter cache exceeds the configured size limit".to_string());
    }
    let cache = serde_json::from_slice::<HashMap<String, String>>(&bytes)
        .map_err(|error| format!("custom filter cache is invalid: {error}"))?;
    let mut size = 0usize;
    for (url, contents) in &cache {
        validate_custom_filter_config(vec![url.clone()], String::new())?;
        if contents.len() > MAX_FILTER_LIST_BYTES {
            return Err("cached custom filter list exceeds the configured size limit".to_string());
        }
        size = size.saturating_add(contents.len());
    }
    if size > MAX_CUSTOM_FILTER_CACHE_BYTES {
        return Err("custom filter cache exceeds the configured size limit".to_string());
    }
    Ok(cache)
}

fn persist_custom_filter_cache(
    directory: &Path,
    custom_lists: &HashMap<String, String>,
    config: &CustomFilterConfig,
) -> Result<(), String> {
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("could not create ad blocker cache directory: {error}"))?;
    let serialized_lists = serde_json::to_vec(custom_lists)
        .map_err(|error| format!("could not encode custom filter cache: {error}"))?;
    if serialized_lists.len() > MAX_CUSTOM_FILTER_CACHE_BYTES {
        return Err("custom filter cache exceeds the configured size limit".to_string());
    }
    let serialized_config = serde_json::to_vec(config)
        .map_err(|error| format!("could not encode custom filter settings: {error}"))?;
    std::fs::write(directory.join("custom-filter-lists.json"), serialized_lists)
        .map_err(|error| format!("could not save custom filter cache: {error}"))?;
    std::fs::write(
        directory.join("custom-filter-config.json"),
        serialized_config,
    )
    .map_err(|error| format!("could not save custom filter settings: {error}"))
}

fn combine_filter_lists(
    lists: &HashMap<String, String>,
    custom_lists: &HashMap<String, String>,
    config: &CustomFilterConfig,
) -> String {
    let mut combined = BUILTIN_FILTERS.to_string();
    for (filename, _) in FILTER_LISTS {
        if let Some(list) = lists.get(filename) {
            combined.push_str(list);
            combined.push('\n');
        }
    }
    for url in &config.list_urls {
        if let Some(list) = custom_lists.get(url) {
            combined.push_str(list);
            combined.push('\n');
        }
    }
    combined.push_str(&config.user_rules);
    combined.push('\n');
    combined
}

fn extract_malicious_domains(lists: &HashMap<String, String>) -> HashSet<String> {
    let Some(list) = lists.get(MALWARE_FILTER_LIST.0) else {
        return HashSet::new();
    };
    list.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('!') || line.starts_with("@@") {
                return None;
            }
            let rule = line.strip_prefix("||")?;
            let raw_host = rule
                .split(|character| matches!(character, '^' | '$' | '/' | '?'))
                .next()?;
            let host = raw_host.strip_prefix("*.").unwrap_or(raw_host);
            if host.is_empty()
                || !host
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.'))
            {
                return None;
            }
            let parsed = Url::parse(&format!("https://{host}/")).ok()?;
            let normalized = parsed.host_str()?.to_ascii_lowercase();
            (normalized == host.to_ascii_lowercase()).then_some(normalized)
        })
        .collect()
}

fn persist_filter_lists_and_build_engine(
    directory: &Path,
    lists: &HashMap<String, String>,
    custom_lists: &HashMap<String, String>,
    config: &CustomFilterConfig,
) -> Result<Engine, String> {
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("could not create ad blocker cache directory: {error}"))?;
    for (filename, _) in FILTER_LISTS {
        if let Some(contents) = lists.get(filename) {
            std::fs::write(directory.join(filename), contents)
                .map_err(|error| format!("could not save {filename}: {error}"))?;
        }
    }
    persist_custom_filter_cache(directory, custom_lists, config)?;
    let engine = build_engine(&combine_filter_lists(lists, custom_lists, config));
    persist_engine(directory, &engine)?;
    Ok(engine)
}

fn persist_engine(directory: &Path, engine: &Engine) -> Result<(), String> {
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("could not create ad blocker cache directory: {error}"))?;
    std::fs::write(directory.join("engine.bin"), engine.serialize())
        .map_err(|error| format!("could not save compiled ad blocker engine: {error}"))
}

fn filter_cache_is_stale(directory: &Path) -> Result<bool, String> {
    let now = SystemTime::now();
    for (filename, _) in FILTER_LISTS {
        let metadata = match std::fs::metadata(directory.join(filename)) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
            Err(error) => return Err(format!("could not inspect {filename}: {error}")),
        };
        if metadata.len() > MAX_FILTER_LIST_BYTES as u64 {
            return Ok(true);
        }
        let modified = metadata
            .modified()
            .map_err(|error| format!("could not read {filename} modification time: {error}"))?;
        if now.duration_since(modified).unwrap_or_default() >= FILTER_UPDATE_INTERVAL {
            return Ok(true);
        }
    }
    Ok(false)
}

fn load_preferences(
    connection: &Connection,
) -> Result<(bool, std::collections::HashSet<String>, CustomFilterConfig), String> {
    let enabled_value = connection
        .query_row(
            "SELECT value FROM settings WHERE key = 'adblock.enabled'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| format!("could not read ad blocker setting: {error}"))?;
    let enabled = match enabled_value.as_deref() {
        None | Some("true") => true,
        Some("false") => false,
        Some(_) => return Err("stored ad blocker preference is invalid".to_string()),
    };
    let mut statement = connection
        .prepare("SELECT host FROM site_permissions WHERE adblock_allowed = 1")
        .map_err(|error| format!("could not read ad blocker allowlist: {error}"))?;
    let allowed_sites = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| format!("could not read ad blocker allowlist: {error}"))?
        .collect::<Result<std::collections::HashSet<_>, _>>()
        .map_err(|error| format!("could not decode ad blocker allowlist: {error}"))?;
    let list_urls = load_filter_setting(connection, "adblock.custom_list_urls")?
        .map(|value| {
            serde_json::from_str::<Vec<String>>(&value).map_err(|error| {
                format!("stored custom filter list configuration is invalid: {error}")
            })
        })
        .transpose()?
        .unwrap_or_default();
    let user_rules = load_filter_setting(connection, "adblock.user_rules")?.unwrap_or_default();
    let custom_filter_config = validate_custom_filter_config(list_urls, user_rules)?;
    Ok((enabled, allowed_sites, custom_filter_config))
}

fn load_filter_setting(connection: &Connection, key: &str) -> Result<Option<String>, String> {
    connection
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .map_err(|error| format!("could not read ad blocker setting {key}: {error}"))
}

fn validate_custom_filter_config(
    list_urls: Vec<String>,
    user_rules: String,
) -> Result<CustomFilterConfig, String> {
    const MAX_CUSTOM_LISTS: usize = 8;
    const MAX_USER_RULE_BYTES: usize = 64 * 1024;
    const MAX_USER_RULE_LINES: usize = 2000;

    if list_urls.len() > MAX_CUSTOM_LISTS {
        return Err(format!(
            "at most {MAX_CUSTOM_LISTS} custom filter lists are allowed"
        ));
    }
    if user_rules.len() > MAX_USER_RULE_BYTES
        || user_rules.contains('\0')
        || user_rules.lines().count() > MAX_USER_RULE_LINES
    {
        return Err("custom filter rules exceed the allowed size or line count".to_string());
    }

    let mut normalized_urls = Vec::with_capacity(list_urls.len());
    for value in list_urls {
        if value.len() > 2048 {
            return Err("custom filter list URL is too long".to_string());
        }
        let mut url = Url::parse(value.trim())
            .map_err(|_| "custom filter list URLs must be valid HTTPS URLs".to_string())?;
        if url.scheme() != "https"
            || url.host().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(
                "custom filter list URLs must be HTTPS without credentials or fragments"
                    .to_string(),
            );
        }
        url.set_fragment(None);
        let normalized = url.to_string();
        if !normalized_urls.contains(&normalized) {
            normalized_urls.push(normalized);
        }
    }
    Ok(CustomFilterConfig {
        list_urls: normalized_urls,
        user_rules,
    })
}

fn normalize_host(host: &str) -> Result<String, String> {
    let host = host.trim();
    if host.is_empty() || host.len() > 253 || host.contains('/') || host.contains('@') {
        return Err("invalid site hostname".to_string());
    }
    let url =
        Url::parse(&format!("https://{host}/")).map_err(|_| "invalid site hostname".to_string())?;
    let normalized = url
        .host_str()
        .ok_or_else(|| "invalid site hostname".to_string())?
        .to_ascii_lowercase();
    if url.port().is_some() || url.username() != "" || url.password().is_some() {
        return Err("site allowlisting accepts hostnames only".to_string());
    }
    Ok(normalized)
}

#[tauri::command]
pub fn adblock_status(
    host: String,
    service: State<'_, AdblockService>,
) -> Result<AdblockStatus, String> {
    service.status(&host)
}

#[tauri::command]
pub async fn adblock_set_enabled(
    enabled: bool,
    service: State<'_, AdblockService>,
    storage: State<'_, StorageService>,
) -> Result<(), String> {
    storage
        .with_connection(move |connection| {
            connection
                .execute(
                    "INSERT INTO settings(key, value) VALUES ('adblock.enabled', ?1)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    [if enabled { "true" } else { "false" }],
                )
                .map_err(|error| format!("could not save ad blocker setting: {error}"))?;
            Ok(())
        })
        .await?;
    service.set_enabled(enabled)
}

#[tauri::command]
pub async fn adblock_set_site_allowed(
    host: String,
    allowed: bool,
    service: State<'_, AdblockService>,
    storage: State<'_, StorageService>,
) -> Result<(), String> {
    let host = normalize_host(&host)?;
    let persisted_host = host.clone();
    storage
        .with_connection(move |connection| {
            if allowed {
                connection
                    .execute(
                        "INSERT INTO site_permissions(host, adblock_allowed) VALUES (?1, 1)
                         ON CONFLICT(host) DO UPDATE SET adblock_allowed = 1",
                        [&persisted_host],
                    )
                    .map_err(|error| format!("could not save site allowlist entry: {error}"))?;
            } else {
                connection
                    .execute(
                        "DELETE FROM site_permissions WHERE host = ?1",
                        [&persisted_host],
                    )
                    .map_err(|error| format!("could not remove site allowlist entry: {error}"))?;
            }
            Ok(())
        })
        .await?;
    service.set_site_allowed(&host, allowed)
}

#[tauri::command]
pub async fn adblock_get_custom_filters(
    service: State<'_, AdblockService>,
    storage: State<'_, StorageService>,
) -> Result<CustomFilterConfig, String> {
    ensure_preferences_loaded(storage.inner(), service.inner()).await?;
    service.custom_filter_config()
}

#[tauri::command]
pub async fn adblock_set_custom_filters(
    list_urls: Vec<String>,
    user_rules: String,
    service: State<'_, AdblockService>,
    storage: State<'_, StorageService>,
) -> Result<(), String> {
    ensure_preferences_loaded(storage.inner(), service.inner()).await?;
    let config = validate_custom_filter_config(list_urls, user_rules)?;
    let urls_value = serde_json::to_string(&config.list_urls)
        .map_err(|error| format!("could not encode custom filter list settings: {error}"))?;
    let rules_value = config.user_rules.clone();
    storage
        .with_connection(move |connection| {
            connection
                .execute(
                    "INSERT INTO settings(key, value)
                     VALUES ('adblock.custom_list_urls', ?1), ('adblock.user_rules', ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    [&urls_value, &rules_value],
                )
                .map_err(|error| format!("could not save custom filter settings: {error}"))?;
            Ok(())
        })
        .await?;
    service.set_custom_filter_config(config)?;

    let directory = service.filter_cache_directory()?;
    let service = service.inner().clone();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| format!("could not create custom filter update client: {error}"))?;
    refresh_filter_lists(&client, &service, &directory).await
}

pub fn attach_to_webview(
    webview: &Webview<tauri::Wry>,
    service: AdblockService,
    privacy: PrivacyService,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::{cell::RefCell, rc::Rc, sync::mpsc};

        use webview2_com::{
            take_pwstr, FrameCreatedEventHandler,
            Microsoft::Web::WebView2::Win32::{
                ICoreWebView2, ICoreWebView2_22, ICoreWebView2_4,
                COREWEBVIEW2_PERMISSION_KIND_CAMERA, COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ,
                COREWEBVIEW2_PERMISSION_KIND_GEOLOCATION, COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
                COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS, COREWEBVIEW2_PERMISSION_STATE_ALLOW,
                COREWEBVIEW2_PERMISSION_STATE_DENY, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
            },
            NavigationStartingEventHandler, PermissionRequestedEventHandler,
            WebMessageReceivedEventHandler, WebResourceRequestedEventHandler,
        };
        use windows::{
            core::{w, Interface, HSTRING, PWSTR},
            Win32::System::Com::IStream,
        };

        let (result_sender, result_receiver) = mpsc::sync_channel(1);
        webview
            .with_webview(move |platform| {
                let result = (|| -> Result<(), String> {
                    unsafe {
                        let controller = platform.controller();
                        let core = controller
                            .CoreWebView2()
                            .map_err(|error| format!("could not access WebView2: {error}"))?;
                        let filter = HSTRING::from("*");
                        match core.cast::<ICoreWebView2_22>() {
                            Ok(webview_22) => webview_22
                                .AddWebResourceRequestedFilterWithRequestSourceKinds(
                                    &filter,
                                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                                    COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
                                )
                                .map_err(|error| {
                                    format!("could not install WebView2 request filter: {error}")
                                })?,
                            Err(_) => core
                                .AddWebResourceRequestedFilter(
                                    &filter,
                                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                                )
                                .map_err(|error| {
                                    format!("could not install WebView2 request filter: {error}")
                                })?,
                        }

                        let environment = platform.environment();
                        let callback_service = service.clone();
                        let callback_privacy = privacy.clone();
                        let permission_privacy = privacy.clone();
                        let cosmetic_service = service;
                        let handler = WebResourceRequestedEventHandler::create(Box::new(
                            move |sender: Option<ICoreWebView2>, args| {
                                let Some(args) = args else {
                                    return Ok(());
                                };
                                let request = args.Request()?;
                                let mut request_uri = PWSTR::null();
                                request.Uri(&mut request_uri)?;
                                let mut request_url = take_pwstr(request_uri);
                                if let Some(upgraded_url) =
                                    callback_privacy.upgrade_request_url(&request_url)
                                {
                                    request.SetUri(&HSTRING::from(upgraded_url.clone()))?;
                                    request_url = upgraded_url;
                                }
                                if !request_url.starts_with("http://")
                                    && !request_url.starts_with("https://")
                                {
                                    return Ok(());
                                }
                                let headers = request.Headers()?;
                                if let Err(error) = headers.SetHeader(w!("DNT"), w!("1")) {
                                    eprintln!("could not set Do Not Track request header: {error}");
                                }
                                if let Err(error) = headers.SetHeader(w!("Sec-GPC"), w!("1")) {
                                    eprintln!(
                                    "could not set Global Privacy Control request header: {error}"
                                );
                                }

                                let mut source_uri = PWSTR::null();
                                let source_url = match sender {
                                    Some(sender) if sender.Source(&mut source_uri).is_ok() => {
                                        take_pwstr(source_uri)
                                    }
                                    _ => String::new(),
                                };
                                if callback_privacy.block_third_party_cookies()
                                    && !source_url.is_empty()
                                    && !crate::privacy::is_same_site_request(
                                        &source_url,
                                        &request_url,
                                    )
                                {
                                    let mut has_cookie = Default::default();
                                    match headers.Contains(w!("Cookie"), &mut has_cookie) {
                                        Ok(()) if has_cookie.as_bool() => {
                                            if let Err(error) = headers.RemoveHeader(w!("Cookie")) {
                                                eprintln!(
                                                    "could not remove cross-site Cookie header: {error}"
                                                );
                                            }
                                        }
                                        Ok(()) => {}
                                        Err(error) => {
                                            eprintln!(
                                                "could not inspect cross-site Cookie header: {error}"
                                            );
                                        }
                                    }
                                }

                                let mut context = Default::default();
                                args.ResourceContext(&mut context)?;
                                let request_type = request_type_for_context(context.0);
                                let mut method = PWSTR::null();
                                request.Method(&mut method)?;
                                let method = take_pwstr(method);
                                if callback_service
                                    .should_block(
                                        &request_url,
                                        &source_url,
                                        request_type,
                                        if method.is_empty() { "GET" } else { &method },
                                    )
                                    .unwrap_or_else(|error| {
                                        eprintln!("ad blocker request check failed: {error}");
                                        false
                                    })
                                {
                                    let response = environment.CreateWebResourceResponse(
                                        None::<&IStream>,
                                        204,
                                        &HSTRING::from("No Content"),
                                        &HSTRING::from("Content-Length: 0"),
                                    )?;
                                    args.SetResponse(&response)?;
                                }
                                Ok(())
                            },
                        ));
                        let mut token = 0;
                        core.add_WebResourceRequested(&handler, &mut token)
                            .map_err(|error| {
                                format!("could not register WebView2 request handler: {error}")
                            })?;

                        let permission_handler =
                            PermissionRequestedEventHandler::create(Box::new(move |_, args| {
                                let Some(args) = args else {
                                    return Ok(());
                                };
                                let mut kind = Default::default();
                                args.PermissionKind(&mut kind)?;
                                let permission = if kind == COREWEBVIEW2_PERMISSION_KIND_CAMERA {
                                    Some("camera")
                                } else if kind == COREWEBVIEW2_PERMISSION_KIND_MICROPHONE {
                                    Some("microphone")
                                } else if kind == COREWEBVIEW2_PERMISSION_KIND_GEOLOCATION {
                                    Some("location")
                                } else if kind == COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS {
                                    Some("notifications")
                                } else if kind == COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ {
                                    Some("clipboard")
                                } else {
                                    None
                                };
                                let Some(permission) = permission else {
                                    return Ok(());
                                };
                                let mut uri = PWSTR::null();
                                args.Uri(&mut uri)?;
                                let allowed = permission_origin_for_url(&take_pwstr(uri))
                                    .and_then(|origin| {
                                        permission_privacy
                                            .site_permission_allowed(&origin, permission)
                                            .map_err(|error| {
                                                eprintln!(
                                                    "could not read site permission: {error}"
                                                );
                                            })
                                            .ok()
                                    })
                                    .unwrap_or(false);
                                args.SetState(if allowed {
                                    COREWEBVIEW2_PERMISSION_STATE_ALLOW
                                } else {
                                    COREWEBVIEW2_PERMISSION_STATE_DENY
                                })?;
                                Ok(())
                            }));
                        let mut permission_token = 0;
                        core.add_PermissionRequested(&permission_handler, &mut permission_token)
                            .map_err(|error| {
                                format!("could not register site permission handler: {error}")
                            })?;

                        let cosmetic_scripts =
                            Rc::new(RefCell::new(CosmeticScriptRegistry::default()));
                        let navigation_service = cosmetic_service.clone();
                        let threat_service = cosmetic_service.clone();
                        let navigation_scripts = cosmetic_scripts.clone();
                        let navigation_handler = NavigationStartingEventHandler::create(Box::new(
                            move |sender, args| {
                                let Some(core) = sender else {
                                    return Ok(());
                                };
                                let Some(args) = args else {
                                    return Ok(());
                                };
                                let mut uri = PWSTR::null();
                                args.Uri(&mut uri)?;
                                let url = take_pwstr(uri);
                                match threat_service.is_malicious_url(&url) {
                                    Ok(true) => {
                                        args.SetCancel(true)?;
                                        core.NavigateToString(&HSTRING::from(THREAT_WARNING_HTML))?;
                                        return Ok(());
                                    }
                                    Ok(false) => {}
                                    Err(error) => {
                                        eprintln!("could not check malware blocklist: {error}");
                                    }
                                }
                                install_cosmetic_script(
                                    &core,
                                    &navigation_service,
                                    &navigation_scripts,
                                    "main",
                                    &url,
                                    true,
                                );
                                Ok(())
                            },
                        ));
                        let mut navigation_token = 0;
                        if let Err(error) =
                            core.add_NavigationStarting(&navigation_handler, &mut navigation_token)
                        {
                            eprintln!("could not register cosmetic navigation handler: {error}");
                        }

                        let message_service = cosmetic_service.clone();
                        let message_scripts = cosmetic_scripts.clone();
                        let message_handler = WebMessageReceivedEventHandler::create(Box::new(
                            move |sender, args| {
                                let Some(core) = sender else {
                                    return Ok(());
                                };
                                let Some(args) = args else {
                                    return Ok(());
                                };
                                let mut source = PWSTR::null();
                                let mut message = PWSTR::null();
                                args.Source(&mut source)?;
                                args.WebMessageAsJson(&mut message)?;
                                let source = take_pwstr(source);
                                let message = take_pwstr(message);
                                if !is_generic_cosmetic_message(&message) {
                                    return Ok(());
                                }
                                match process_generic_cosmetic_message(
                                    &message_service,
                                    &message_scripts,
                                    "main",
                                    &source,
                                    &message,
                                ) {
                                    Ok(Some(reply)) => {
                                        core.PostWebMessageAsJson(&HSTRING::from(reply))?;
                                    }
                                    Ok(None) => {}
                                    Err(error) => {
                                        log_cosmetic_message_error(&message_scripts, "main", &error)
                                    }
                                }
                                Ok(())
                            },
                        ));
                        let mut message_token = 0;
                        if let Err(error) =
                            core.add_WebMessageReceived(&message_handler, &mut message_token)
                        {
                            eprintln!(
                                "could not register generic cosmetic message handler: {error}"
                            );
                        }

                        match core.cast::<ICoreWebView2_4>() {
                            Ok(core_4) => {
                                let frame_service = cosmetic_service.clone();
                                let frame_scripts = cosmetic_scripts.clone();
                                let frame_handler = FrameCreatedEventHandler::create(Box::new(
                                    move |sender, args| {
                                        let Some(core) = sender else {
                                            return Ok(());
                                        };
                                        let Some(args) = args else {
                                            return Ok(());
                                        };
                                        let frame = args.Frame()?;
                                        if let Err(error) = attach_frame_cosmetics(
                                            &core,
                                            frame,
                                            frame_service.clone(),
                                            frame_scripts.clone(),
                                        ) {
                                            eprintln!(
                                                "could not attach cosmetic frame handler: {error}"
                                            );
                                        }
                                        Ok(())
                                    },
                                ));
                                let mut frame_token = 0;
                                if let Err(error) =
                                    core_4.add_FrameCreated(&frame_handler, &mut frame_token)
                                {
                                    eprintln!("could not register cosmetic frame handler: {error}");
                                }
                            }
                            Err(error) => {
                                eprintln!("WebView2 frame cosmetics are unavailable: {error}");
                            }
                        }

                        Ok(())
                    }
                })();
                if result_sender.send(result).is_err() {
                    eprintln!("could not return WebView2 interception setup result");
                }
            })
            .map_err(|error| format!("could not access native webview: {error}"))?;
        result_receiver.recv().map_err(|error| {
            format!("could not receive WebView2 interception setup result: {error}")
        })?
    }

    #[cfg(not(windows))]
    {
        let _ = (webview, service, privacy);
        Err("native request interception is currently supported on Windows only".to_string())
    }
}

#[cfg(windows)]
fn install_cosmetic_script(
    core: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
    service: &AdblockService,
    registered_scripts: &CosmeticScriptRegistryRef,
    key: &str,
    page_url: &str,
    clear_all: bool,
) {
    use webview2_com::AddScriptToExecuteOnDocumentCreatedCompletedHandler;
    use windows::core::HSTRING;

    let (old_ids, epoch, generation, allow_generic) = {
        let mut scripts = registered_scripts.borrow_mut();
        if clear_all {
            scripts.epoch = scripts.epoch.wrapping_add(1);
            let old_ids = scripts
                .script_ids
                .drain()
                .map(|(_, id)| id)
                .collect::<Vec<_>>();
            scripts.generic_quotas.clear();
            scripts.generic_batches_total = 0;
            scripts.generic_tokens_total = 0;
            scripts.generations.clear();
            let epoch = scripts.epoch;
            scripts.next_generation = scripts.next_generation.wrapping_add(1);
            let generation = scripts.next_generation;
            scripts.generations.insert(key.to_string(), generation);
            let allow_generic = scripts.generic_quotas.len() < MAX_COSMETIC_FRAMES;
            if allow_generic {
                scripts.generic_quotas.insert(
                    key.to_string(),
                    GenericCosmeticQuota {
                        page_key: cosmetic_page_key(page_url),
                        ..Default::default()
                    },
                );
            }
            (old_ids, epoch, generation, allow_generic)
        } else {
            let old_ids = scripts
                .script_ids
                .remove(key)
                .into_iter()
                .collect::<Vec<_>>();
            let epoch = scripts.epoch;
            scripts.next_generation = scripts.next_generation.wrapping_add(1);
            let generation = scripts.next_generation;
            scripts.generations.insert(key.to_string(), generation);
            scripts.generic_quotas.remove(key);
            let allow_generic = scripts.generic_quotas.len() < MAX_COSMETIC_FRAMES;
            if allow_generic {
                scripts.generic_quotas.insert(
                    key.to_string(),
                    GenericCosmeticQuota {
                        page_key: cosmetic_page_key(page_url),
                        ..Default::default()
                    },
                );
            }
            (old_ids, epoch, generation, allow_generic)
        }
    };
    for id in old_ids {
        if let Err(error) =
            unsafe { core.RemoveScriptToExecuteOnDocumentCreated(&HSTRING::from(id)) }
        {
            eprintln!("could not remove stale cosmetic script: {error}");
        }
    }

    let script = match service.cosmetic_script(page_url, allow_generic) {
        Ok(Some(script)) => script,
        Ok(None) => return,
        Err(error) => {
            eprintln!("could not create cosmetic script: {error}");
            return;
        }
    };

    let key = key.to_string();
    let scripts_for_callback = registered_scripts.clone();
    let core_for_callback = core.clone();
    let handler =
        AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(move |result, id| {
            if let Err(error) = result {
                eprintln!("could not register cosmetic document-start script: {error}");
                return Ok(());
            }
            let is_current = {
                let mut scripts = scripts_for_callback.borrow_mut();
                if scripts.epoch == epoch
                    && scripts.generations.get(&key).copied() == Some(generation)
                {
                    scripts.script_ids.insert(key, id.clone());
                    true
                } else {
                    false
                }
            };
            if !is_current {
                if let Err(error) = unsafe {
                    core_for_callback.RemoveScriptToExecuteOnDocumentCreated(&HSTRING::from(id))
                } {
                    eprintln!("could not remove superseded cosmetic script: {error}");
                }
            }
            Ok(())
        }));
    let script = HSTRING::from(script);
    if let Err(error) = unsafe { core.AddScriptToExecuteOnDocumentCreated(&script, &handler) } {
        eprintln!("could not register cosmetic document-start script: {error}");
    }
}

#[cfg(windows)]
fn attach_frame_cosmetics(
    core: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
    frame: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Frame,
    service: AdblockService,
    registered_scripts: CosmeticScriptRegistryRef,
) -> windows::core::Result<()> {
    use webview2_com::{
        take_pwstr, FrameChildFrameCreatedEventHandler, FrameDestroyedEventHandler,
        FrameNavigationStartingEventHandler, FrameWebMessageReceivedEventHandler,
        Microsoft::Web::WebView2::Win32::{ICoreWebView2Frame2, ICoreWebView2Frame7},
    };
    use windows::core::{Interface, PWSTR};

    let frame_id = format!("frame:{:x}", frame.as_raw() as usize);

    let frame_2 = match frame.cast::<ICoreWebView2Frame2>() {
        Ok(frame_2) => frame_2,
        Err(error) => {
            eprintln!("WebView2 frame navigation events are unavailable: {error}");
            return Ok(());
        }
    };
    let navigation_core = core.clone();
    let navigation_service = service.clone();
    let navigation_scripts = registered_scripts.clone();
    let navigation_key = frame_id.clone();
    let navigation_handler =
        FrameNavigationStartingEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else {
                return Ok(());
            };
            let mut uri = PWSTR::null();
            unsafe { args.Uri(&mut uri)? };
            install_cosmetic_script(
                &navigation_core,
                &navigation_service,
                &navigation_scripts,
                &navigation_key,
                &take_pwstr(uri),
                false,
            );
            Ok(())
        }));
    let mut navigation_token = 0;
    unsafe { frame_2.add_NavigationStarting(&navigation_handler, &mut navigation_token)? };

    let message_service = service.clone();
    let message_registry = registered_scripts.clone();
    let message_key = frame_id.clone();
    let message_handler =
        FrameWebMessageReceivedEventHandler::create(Box::new(move |sender, args| {
            let Some(sender) = sender else {
                return Ok(());
            };
            let Some(args) = args else {
                return Ok(());
            };
            let mut source = PWSTR::null();
            let mut message = PWSTR::null();
            unsafe {
                args.Source(&mut source)?;
                args.WebMessageAsJson(&mut message)?;
            }
            let source = take_pwstr(source);
            let message = take_pwstr(message);
            if !is_generic_cosmetic_message(&message) {
                return Ok(());
            }
            match process_generic_cosmetic_message(
                &message_service,
                &message_registry,
                &message_key,
                &source,
                &message,
            ) {
                Ok(Some(reply)) => {
                    let message_target = sender.cast::<ICoreWebView2Frame2>()?;
                    unsafe {
                        message_target
                            .PostWebMessageAsJson(&windows::core::HSTRING::from(reply))?;
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    log_cosmetic_message_error(&message_registry, &message_key, &error);
                }
            }
            Ok(())
        }));
    let mut message_token = 0;
    unsafe { frame_2.add_WebMessageReceived(&message_handler, &mut message_token)? };

    let destroy_core = core.clone();
    let destroy_scripts = registered_scripts.clone();
    let destroy_key = frame_id.clone();
    let destroyed_handler = FrameDestroyedEventHandler::create(Box::new(move |_, _| {
        remove_cosmetic_script(&destroy_core, &destroy_scripts, &destroy_key);
        Ok(())
    }));
    let mut destroyed_token = 0;
    unsafe { frame.add_Destroyed(&destroyed_handler, &mut destroyed_token)? };

    if let Ok(frame_7) = frame.cast::<ICoreWebView2Frame7>() {
        let nested_core = core.clone();
        let nested_service = service.clone();
        let nested_scripts = registered_scripts.clone();
        let child_handler = FrameChildFrameCreatedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else {
                return Ok(());
            };
            let child = unsafe { args.Frame()? };
            if let Err(error) = attach_frame_cosmetics(
                &nested_core,
                child,
                nested_service.clone(),
                nested_scripts.clone(),
            ) {
                eprintln!("could not attach nested cosmetic frame handler: {error}");
            }
            Ok(())
        }));
        let mut child_token = 0;
        unsafe {
            if let Err(error) = frame_7.add_FrameCreated(&child_handler, &mut child_token) {
                eprintln!("could not register nested cosmetic frame handler: {error}");
            }
        }
    }

    Ok(())
}

#[cfg(windows)]
fn remove_cosmetic_script(
    core: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
    registered_scripts: &CosmeticScriptRegistryRef,
    key: &str,
) {
    use windows::core::HSTRING;

    let id = {
        let mut scripts = registered_scripts.borrow_mut();
        scripts.generations.remove(key);
        scripts.generic_quotas.remove(key);
        scripts.script_ids.remove(key)
    };
    if let Some(id) = id {
        if let Err(error) =
            unsafe { core.RemoveScriptToExecuteOnDocumentCreated(&HSTRING::from(id)) }
        {
            eprintln!("could not remove cosmetic script for closed frame: {error}");
        }
    }
}

#[cfg(windows)]
fn request_type_for_context(context: i32) -> &'static str {
    use webview2_com::Microsoft::Web::WebView2::Win32::*;

    match context {
        value if value == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT.0 => "document",
        value if value == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET.0 => "stylesheet",
        value if value == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE.0 => "image",
        value if value == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT.0 => "script",
        value if value == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT.0 => "font",
        value if value == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA.0 => "media",
        value if value == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING.0 => "ping",
        value
            if value == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH.0
                || value == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST.0 =>
        {
            "xmlhttprequest"
        }
        value if value == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_WEBSOCKET.0 => "websocket",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_non_cosmetic_webview_messages() {
        let ipc_payload = serde_json::to_string(r#"{"cmd":"invoke","payload":{}}"#)
            .expect("encode string-valued IPC payload");
        let cosmetic_payload = serde_json::json!({
            "kind": GENERIC_COSMETIC_MESSAGE,
            "classes": [],
            "ids": []
        })
        .to_string();

        assert!(!is_generic_cosmetic_message(&ipc_payload));
        assert!(!is_generic_cosmetic_message(r#"{"kind":"other"}"#));
        assert!(is_generic_cosmetic_message(&cosmetic_payload));
    }

    #[test]
    fn malware_domain_rules_match_subdomains_without_suffix_confusion() {
        let domains = extract_malicious_domains(&HashMap::from([(
            MALWARE_FILTER_LIST.0.to_string(),
            "||infected.example^\n||*.bad.test^$document\n@@||safe.example^\n".to_string(),
        )]));
        assert!(domains.contains("infected.example"));
        assert!(domains.contains("bad.test"));
        assert!(!domains.contains("safe.example"));

        let service = AdblockService::new();
        service
            .replace_malicious_domains(domains)
            .expect("install malware domains");
        assert!(service
            .is_malicious_url("https://cdn.infected.example/path")
            .expect("check malware subdomain"));
        assert!(!service
            .is_malicious_url("https://infected.example.attacker.test/")
            .expect("avoid suffix collision"));
    }

    #[test]
    fn blocks_network_filters_and_honors_filter_exceptions() {
        let service = AdblockService(Arc::new(AdblockInner {
            engine: RwLock::new(build_engine(
                "||ads.example.test^\n\
                 ||api.example.test^$xmlhttprequest\n\
                 @@||ads.example.test^$domain=publisher.example.test",
            )),
            enabled: AtomicBool::new(true),
            allowed_sites: RwLock::new(std::collections::HashSet::new()),
            blocked_counts: Mutex::new(HashMap::new()),
            malicious_domains: RwLock::new(HashSet::new()),
            custom_filter_config: RwLock::new(CustomFilterConfig::default()),
            filter_cache_directory: RwLock::new(None),
            filter_refresh_lock: tokio::sync::Mutex::new(()),
            preference_lock: Mutex::new(()),
            global_preference_changed: AtomicBool::new(false),
            modified_sites: Mutex::new(std::collections::HashSet::new()),
            preferences_loaded: AtomicBool::new(false),
        }));

        assert!(service
            .should_block(
                "https://ads.example.test/banner.js",
                "https://news.example.test/",
                "script",
                "GET",
            )
            .expect("check network filter"));
        assert!(!service
            .should_block(
                "https://ads.example.test/banner.js",
                "https://publisher.example.test/",
                "script",
                "GET",
            )
            .expect("check exception filter"));
        assert!(service
            .should_block(
                "https://api.example.test/collect",
                "https://news.example.test/",
                "xmlhttprequest",
                "POST",
            )
            .expect("check XMLHttpRequest filter"));
        assert!(!service
            .should_block(
                "https://api.example.test/collect",
                "https://news.example.test/",
                "script",
                "GET",
            )
            .expect("check request-type specificity"));
    }

    #[test]
    fn cosmetic_filters_are_available_from_the_engine() {
        let engine = build_engine("news.example.test##.ad-banner");
        let resources = engine.url_cosmetic_resources("https://news.example.test/article");
        assert!(resources.hide_selectors.contains(".ad-banner"));
    }

    #[test]
    fn bundled_ublock_scriptlets_are_resolved_and_injected() {
        let engine = build_engine(
            "news.example.test##+js(abort-current-script, foo, bar)\n\
             news.example.test##+js(set-constant, eefiScriptletProbe, true)",
        );
        let resources = engine.url_cosmetic_resources("https://news.example.test/article");

        assert!(resources.injected_script.contains("const target = 'foo';"));
        assert!(resources.injected_script.contains("const needle = 'bar';"));
        assert!(resources
            .injected_script
            .contains("const chain = 'eefiScriptletProbe';"));
        assert!(resources.injected_script.contains("let cValue = 'true';"));
    }

    #[test]
    fn procedural_text_filters_are_applied_with_bounded_dom_actions() {
        let service = AdblockService(Arc::new(AdblockInner {
            engine: RwLock::new(build_engine(
                "news.example.test##.ad:has-text(Sponsored):remove()\n\
                 news.example.test##.notice:has-text(Advertisement):remove-class(advert)\n\
                 news.example.test##.box:has-text(Notice):style(color: red)\n\
                 news.example.test##.widget:has-text(Promo):remove-attr(data-ad)\n\
                 news.example.test##.ad:has-text(/sponsored/i):remove()",
            )),
            enabled: AtomicBool::new(true),
            allowed_sites: RwLock::new(HashSet::new()),
            blocked_counts: Mutex::new(HashMap::new()),
            malicious_domains: RwLock::new(HashSet::new()),
            custom_filter_config: RwLock::new(CustomFilterConfig::default()),
            filter_cache_directory: RwLock::new(None),
            filter_refresh_lock: tokio::sync::Mutex::new(()),
            preference_lock: Mutex::new(()),
            global_preference_changed: AtomicBool::new(false),
            modified_sites: Mutex::new(HashSet::new()),
            preferences_loaded: AtomicBool::new(false),
        }));

        let script = service
            .cosmetic_script("https://news.example.test/article", false)
            .expect("build procedural cosmetic script")
            .expect("procedural filters exist");
        assert!(script.contains("Sponsored"));
        assert!(script.contains("Advertisement"));
        assert!(script.contains("Notice"));
        assert!(script.contains("Promo"));
        assert!(script.contains("element.remove()"));
        assert!(script.contains("element.classList.remove(action.arg)"));
        assert!(script.contains(r#""type":"style""#));
        assert!(script.contains(r#""type":"remove-attr""#));
        assert!(script
            .contains(r#""selector":".ad","hasText":["Sponsored"],"action":{"type":"remove"}"#));
        assert!(!script.contains("/sponsored/i"));
        assert!(script.contains("requestAnimationFrame"));
        assert!(!script.contains("__EEFI_RULES__"));
    }

    #[test]
    fn generic_cosmetics_respect_site_exceptions_and_message_source() {
        let url = "https://news.example.test/article";
        let engine = build_engine("##.ad\n##.sponsor\nnews.example.test#@#.sponsor");
        let resources = engine.url_cosmetic_resources(url);
        assert!(!resources.generichide);
        let selectors = engine.hidden_class_id_selectors(
            ["ad", "sponsor"],
            std::iter::empty::<&str>(),
            &resources.exceptions,
        );
        assert!(selectors.contains(&".ad".to_string()));
        assert!(!selectors.contains(&".sponsor".to_string()));

        let service = AdblockService(Arc::new(AdblockInner {
            engine: RwLock::new(engine),
            enabled: AtomicBool::new(true),
            allowed_sites: RwLock::new(HashSet::new()),
            blocked_counts: Mutex::new(HashMap::new()),
            malicious_domains: RwLock::new(HashSet::new()),
            custom_filter_config: RwLock::new(CustomFilterConfig::default()),
            filter_cache_directory: RwLock::new(None),
            filter_refresh_lock: tokio::sync::Mutex::new(()),
            preference_lock: Mutex::new(()),
            global_preference_changed: AtomicBool::new(false),
            modified_sites: Mutex::new(HashSet::new()),
            preferences_loaded: AtomicBool::new(false),
        }));
        let registry = std::rc::Rc::new(std::cell::RefCell::new(CosmeticScriptRegistry::default()));
        registry.borrow_mut().generic_quotas.insert(
            "main".to_string(),
            GenericCosmeticQuota {
                page_key: cosmetic_page_key(url),
                ..Default::default()
            },
        );
        let message = r#"{"kind":"eefi-generic-cosmetic-v1","classes":["ad","sponsor"],"ids":[]}"#;
        let reply = process_generic_cosmetic_message(&service, &registry, "main", url, message)
            .expect("process generic cosmetic message")
            .expect("matching generic selector");
        assert!(reply.contains(".ad"));
        assert!(!reply.contains(".sponsor"));

        assert!(process_generic_cosmetic_message(
            &service,
            &registry,
            "main",
            "https://news.example.test/different-page",
            message,
        )
        .expect("ignore stale page message")
        .is_none());

        assert!(process_generic_cosmetic_message(
            &service,
            &registry,
            "main",
            url,
            &" ".repeat(MAX_COSMETIC_MESSAGE_BYTES + 1),
        )
        .is_err());
        for _ in 1..MAX_COSMETIC_BATCHES {
            assert!(
                process_generic_cosmetic_message(&service, &registry, "main", url, message)
                    .expect("process repeated batch")
                    .is_none()
            );
        }
        assert!(
            process_generic_cosmetic_message(&service, &registry, "main", url, message)
                .expect("ignore batch beyond quota")
                .is_none()
        );
        assert_eq!(
            registry.borrow().generic_quotas["main"].batches,
            MAX_COSMETIC_BATCHES
        );
    }

    #[test]
    fn cosmetic_scripts_are_early_url_scoped_and_respect_site_allowlisting() {
        let service = AdblockService(Arc::new(AdblockInner {
            engine: RwLock::new(build_engine(
                "news.example.test##.ad-banner\nnews.example.test##[data-label=\"ad\"]",
            )),
            enabled: AtomicBool::new(true),
            allowed_sites: RwLock::new(std::collections::HashSet::new()),
            blocked_counts: Mutex::new(HashMap::new()),
            malicious_domains: RwLock::new(HashSet::new()),
            custom_filter_config: RwLock::new(CustomFilterConfig::default()),
            filter_cache_directory: RwLock::new(None),
            filter_refresh_lock: tokio::sync::Mutex::new(()),
            preference_lock: Mutex::new(()),
            global_preference_changed: AtomicBool::new(false),
            modified_sites: Mutex::new(std::collections::HashSet::new()),
            preferences_loaded: AtomicBool::new(false),
        }));

        let url = "https://news.example.test/article?campaign=summer";
        let script = service
            .cosmetic_script(url, true)
            .expect("build document-start script")
            .expect("cosmetic resources exist");
        assert!(script.contains(".ad-banner { display: none !important; }"));
        assert!(script.contains("data-label=\\\"ad\\\""));
        assert!(script.contains("u.origin!==t.origin"));
        assert!(script.contains("search:\"?campaign=summer\""));
        assert!(script.contains("MutationObserver"));
        assert!(script.contains("window.chrome&&window.chrome.webview"));
        assert!(script.contains("window.__eefiCosmeticRootsV1.subscribe"));
        assert!(script.contains("Element.prototype.attachShadow"));
        assert!(script.contains("new MutationObserver(()=>install(root))"));

        service
            .set_site_allowed("news.example.test", true)
            .expect("allowlist site");
        assert!(service
            .cosmetic_script(url, true)
            .expect("check allowlisted cosmetics")
            .is_none());
        service
            .set_site_allowed("news.example.test", false)
            .expect("remove allowlist entry");
        service.set_enabled(false).expect("disable ad blocker");
        assert!(service
            .cosmetic_script(url, true)
            .expect("check disabled cosmetics")
            .is_none());
    }

    #[cfg(windows)]
    #[test]
    fn webview2_resource_contexts_map_to_adblock_request_types() {
        use webview2_com::Microsoft::Web::WebView2::Win32::*;

        let cases = [
            (COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT.0, "document"),
            (COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET.0, "stylesheet"),
            (COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE.0, "image"),
            (COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT.0, "script"),
            (COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT.0, "font"),
            (COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA.0, "media"),
            (COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING.0, "ping"),
            (COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH.0, "xmlhttprequest"),
            (COREWEBVIEW2_WEB_RESOURCE_CONTEXT_WEBSOCKET.0, "websocket"),
            (
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST.0,
                "xmlhttprequest",
            ),
            (COREWEBVIEW2_WEB_RESOURCE_CONTEXT_EVENT_SOURCE.0, "other"),
            (COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MANIFEST.0, "other"),
            (COREWEBVIEW2_WEB_RESOURCE_CONTEXT_TEXT_TRACK.0, "other"),
            (COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SIGNED_EXCHANGE.0, "other"),
            (
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_CSP_VIOLATION_REPORT.0,
                "other",
            ),
            (COREWEBVIEW2_WEB_RESOURCE_CONTEXT_OTHER.0, "other"),
        ];
        for (context, expected) in cases {
            assert_eq!(request_type_for_context(context), expected);
        }
        assert_eq!(request_type_for_context(i32::MAX), "other");
    }

    #[test]
    fn site_allowlist_and_global_toggle_are_scoped_to_the_requesting_site() {
        let service = AdblockService(Arc::new(AdblockInner {
            engine: RwLock::new(build_engine("||ads.example.test^")),
            enabled: AtomicBool::new(true),
            allowed_sites: RwLock::new(std::collections::HashSet::new()),
            blocked_counts: Mutex::new(HashMap::new()),
            malicious_domains: RwLock::new(HashSet::new()),
            custom_filter_config: RwLock::new(CustomFilterConfig::default()),
            filter_cache_directory: RwLock::new(None),
            filter_refresh_lock: tokio::sync::Mutex::new(()),
            preference_lock: Mutex::new(()),
            global_preference_changed: AtomicBool::new(false),
            modified_sites: Mutex::new(std::collections::HashSet::new()),
            preferences_loaded: AtomicBool::new(false),
        }));
        service
            .set_site_allowed("news.example.test", true)
            .expect("allowlist site");
        assert!(!service
            .should_block(
                "https://ads.example.test/ad.js",
                "https://news.example.test/",
                "script",
                "GET",
            )
            .expect("check allowlisted site"));
        assert!(service
            .should_block(
                "https://ads.example.test/ad.js",
                "https://other.example.test/",
                "script",
                "GET",
            )
            .expect("check other site"));
        service.set_enabled(false).expect("disable ad blocker");
        assert!(!service
            .should_block(
                "https://ads.example.test/ad.js",
                "https://other.example.test/",
                "script",
                "GET",
            )
            .expect("check globally disabled blocker"));
    }

    #[test]
    fn a_user_change_wins_over_a_late_preference_restore() {
        let service = AdblockService::new();
        service.set_enabled(false).expect("disable ad blocker");
        service
            .set_site_allowed("override.example.test", false)
            .expect("preserve explicit site setting");
        service
            .set_site_allowed("local.example.test", true)
            .expect("allow local site");
        let persisted_sites = [
            "override.example.test".to_string(),
            "saved.example.test".to_string(),
        ]
        .into_iter()
        .collect();
        service
            .restore_preferences(true, persisted_sites, CustomFilterConfig::default())
            .expect("restore stale preferences");
        assert!(!service.0.enabled.load(Ordering::Relaxed));
        let sites = service.0.allowed_sites.read().expect("allowlist lock");
        assert!(sites.contains("local.example.test"));
        assert!(sites.contains("saved.example.test"));
        assert!(!sites.contains("override.example.test"));
    }

    #[test]
    fn host_validation_rejects_urls_and_ports() {
        assert!(normalize_host("example.test").is_ok());
        assert!(normalize_host("https://example.test/").is_err());
        assert!(normalize_host("example.test:8443").is_err());
    }

    #[test]
    fn custom_filter_configuration_accepts_only_bounded_https_sources() {
        let config = validate_custom_filter_config(
            vec![
                "https://filters.example.test/list.txt".to_string(),
                "https://filters.example.test/list.txt".to_string(),
            ],
            "||custom-rule.example.test^".to_string(),
        )
        .expect("validate custom filters");
        assert_eq!(config.list_urls.len(), 1);
        assert!(config.user_rules.contains("custom-rule.example.test"));
        assert!(validate_custom_filter_config(
            vec!["http://filters.example.test/list.txt".to_string()],
            String::new(),
        )
        .is_err());
        assert!(validate_custom_filter_config(
            vec!["https://user:pass@filters.example.test/list.txt".to_string()],
            String::new(),
        )
        .is_err());
    }

    #[test]
    fn status_without_a_page_host_still_reports_the_global_setting() {
        let service = AdblockService::new();
        service.set_enabled(false).expect("disable ad blocker");
        let status = service.status("").expect("query global status");
        assert!(!status.enabled);
        assert!(!status.site_allowed);
        assert_eq!(status.blocked_count, 0);
    }

    #[test]
    fn compiled_filter_engine_round_trips_through_the_cache() {
        let directory = tempfile::tempdir().expect("temporary filter cache");
        let custom_url = "https://filters.example.test/custom.txt".to_string();
        let config = CustomFilterConfig {
            list_urls: vec![custom_url.clone()],
            user_rules: "||user-rule.example.test^\n".to_string(),
        };
        let custom_lists =
            HashMap::from([(custom_url, "||custom-list.example.test^\n".to_string())]);
        let mut lists = HashMap::new();
        lists.insert(
            "easylist.txt".to_string(),
            "||cached-ad.example.test^\n\
             news.example.test##+js(abort-current-script, foo, bar)"
                .to_string(),
        );
        lists.insert("easyprivacy.txt".to_string(), String::new());
        lists.insert("ublock-filters.txt".to_string(), String::new());
        lists.insert(MALWARE_FILTER_LIST.0.to_string(), String::new());
        persist_filter_lists_and_build_engine(directory.path(), &lists, &custom_lists, &config)
            .expect("compile and persist filter engine");

        assert!(compiled_engine_is_current(directory.path(), &config));
        assert!(!compiled_engine_is_current(
            directory.path(),
            &CustomFilterConfig::default()
        ));
        let engine =
            load_cached_engine(directory.path(), &config).expect("load compiled filter engine");
        for host in [
            "cached-ad.example.test",
            "custom-list.example.test",
            "user-rule.example.test",
        ] {
            let request = Request::new(
                &format!("https://{host}/banner"),
                "https://publisher.example.test/",
                "image",
                "GET",
            )
            .expect("construct filter request");
            assert!(
                engine.check_network_request(&request).should_block(),
                "{host}"
            );
        }
        let cosmetic = engine.url_cosmetic_resources("https://news.example.test/article");
        assert!(cosmetic.injected_script.contains("const target = 'foo';"));
    }
}
