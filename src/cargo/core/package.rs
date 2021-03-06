use std::cell::{Ref, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::hash;
use std::path::{Path, PathBuf};

use semver::Version;

use core::{Dependency, Manifest, PackageId, SourceId, Target, TargetKind};
use core::{Summary, Metadata, SourceMap};
use ops;
use util::{CargoResult, Config, LazyCell, ChainError, internal, human, lev_distance};
use rustc_serialize::{Encoder,Encodable};

/// Information about a package that is available somewhere in the file system.
///
/// A package is a `Cargo.toml` file plus all the files that are part of it.
// TODO: Is manifest_path a relic?
#[derive(Clone, Debug)]
pub struct Package {
    // The package's manifest
    manifest: Manifest,
    // The root of the package
    manifest_path: PathBuf,
}

#[derive(RustcEncodable)]
struct SerializedPackage<'a> {
    name: &'a str,
    version: &'a str,
    id: &'a PackageId,
    source: &'a SourceId,
    dependencies: &'a [Dependency],
    targets: &'a [Target],
    features: &'a HashMap<String, Vec<String>>,
    manifest_path: &'a str,
}

impl Encodable for Package {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        let summary = self.manifest.summary();
        let package_id = summary.package_id();

        SerializedPackage {
            name: &package_id.name(),
            version: &package_id.version().to_string(),
            id: package_id,
            source: summary.source_id(),
            dependencies: summary.dependencies(),
            targets: &self.manifest.targets(),
            features: summary.features(),
            manifest_path: &self.manifest_path.display().to_string(),
        }.encode(s)
    }
}

impl Package {
    pub fn new(manifest: Manifest,
               manifest_path: &Path) -> Package {
        Package {
            manifest: manifest,
            manifest_path: manifest_path.to_path_buf(),
        }
    }

    pub fn for_path(manifest_path: &Path, config: &Config) -> CargoResult<Package> {
        let path = manifest_path.parent().unwrap();
        let source_id = try!(SourceId::for_path(path));
        let (pkg, _) = try!(ops::read_package(&manifest_path, &source_id,
                                              config));
        Ok(pkg)
    }

    pub fn dependencies(&self) -> &[Dependency] { self.manifest.dependencies() }
    pub fn manifest(&self) -> &Manifest { &self.manifest }
    pub fn manifest_path(&self) -> &Path { &self.manifest_path }
    pub fn name(&self) -> &str { self.package_id().name() }
    pub fn package_id(&self) -> &PackageId { self.manifest.package_id() }
    pub fn root(&self) -> &Path { self.manifest_path.parent().unwrap() }
    pub fn summary(&self) -> &Summary { self.manifest.summary() }
    pub fn targets(&self) -> &[Target] { self.manifest().targets() }
    pub fn version(&self) -> &Version { self.package_id().version() }
    pub fn authors(&self) -> &Vec<String> { &self.manifest.metadata().authors }
    pub fn publish(&self) -> bool { self.manifest.publish() }

    pub fn has_custom_build(&self) -> bool {
        self.targets().iter().any(|t| t.is_custom_build())
    }

    pub fn generate_metadata(&self) -> Metadata {
        self.package_id().generate_metadata()
    }

    pub fn find_closest_target(&self, target: &str, kind: TargetKind) -> Option<&Target> {
        let targets = self.targets();

        let matches = targets.iter().filter(|t| *t.kind() == kind)
                                    .map(|t| (lev_distance(target, t.name()), t))
                                    .filter(|&(d, _)| d < 4);
        matches.min_by_key(|t| t.0).map(|t| t.1)
    }
}

impl fmt::Display for Package {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.summary().package_id())
    }
}

impl PartialEq for Package {
    fn eq(&self, other: &Package) -> bool {
        self.package_id() == other.package_id()
    }
}

impl Eq for Package {}

impl hash::Hash for Package {
    fn hash<H: hash::Hasher>(&self, into: &mut H) {
        self.package_id().hash(into)
    }
}

pub struct PackageSet<'cfg> {
    packages: Vec<(PackageId, LazyCell<Package>)>,
    sources: RefCell<SourceMap<'cfg>>,
}

impl<'cfg> PackageSet<'cfg> {
    pub fn new(package_ids: &[PackageId],
               sources: SourceMap<'cfg>) -> PackageSet<'cfg> {
        PackageSet {
            packages: package_ids.iter().map(|id| {
                (id.clone(), LazyCell::new())
            }).collect(),
            sources: RefCell::new(sources),
        }
    }

    pub fn package_ids<'a>(&'a self) -> Box<Iterator<Item=&'a PackageId> + 'a> {
        Box::new(self.packages.iter().map(|&(ref p, _)| p))
    }

    pub fn get(&self, id: &PackageId) -> CargoResult<&Package> {
        let slot = try!(self.packages.iter().find(|p| p.0 == *id).chain_error(|| {
            internal(format!("couldn't find `{}` in package set", id))
        }));
        let slot = &slot.1;
        if let Some(pkg) = slot.borrow() {
            return Ok(pkg)
        }
        let mut sources = self.sources.borrow_mut();
        let source = try!(sources.get_mut(id.source_id()).chain_error(|| {
            internal(format!("couldn't find source for `{}`", id))
        }));
        let pkg = try!(source.download(id).chain_error(|| {
            human("unable to get packages from source")
        }));
        assert!(slot.fill(pkg).is_ok());
        Ok(slot.borrow().unwrap())
    }

    pub fn sources(&self) -> Ref<SourceMap<'cfg>> {
        self.sources.borrow()
    }
}
