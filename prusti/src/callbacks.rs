use crate::verifier::verify;
use prusti_common::config;
use prusti_interface::{
    environment::{mir_storage, Environment},
    specs,
};
use prusti_rustc_interface::{
    driver::Compilation,
    hir::def_id::LocalDefId,
    interface::{interface::Compiler, Config, Queries},
    middle::ty::{
        self,
        query::{query_values::mir_borrowck, ExternProviders, Providers},
        TyCtxt,
    },
    session::Session,
};

#[derive(Default)]
pub struct PrustiCompilerCalls;

#[allow(clippy::needless_lifetimes)]
fn mir_borrowck<'tcx>(tcx: TyCtxt<'tcx>, def_id: LocalDefId) -> mir_borrowck<'tcx> {
    let body_with_facts = prusti_rustc_interface::borrowck::consumers::get_body_with_borrowck_facts(
        tcx,
        ty::WithOptConstParam::unknown(def_id),
    );
    // SAFETY: This is safe because we are feeding in the same `tcx` that is
    // going to be used as a witness when pulling out the data.
    unsafe {
        mir_storage::store_mir_body(tcx, def_id, body_with_facts);
    }
    let mut providers = Providers::default();
    prusti_rustc_interface::borrowck::provide(&mut providers);
    let original_mir_borrowck = providers.mir_borrowck;
    original_mir_borrowck(tcx, def_id)
}

fn override_queries(_session: &Session, local: &mut Providers, _external: &mut ExternProviders) {
    local.mir_borrowck = mir_borrowck;
}

impl prusti_rustc_interface::driver::Callbacks for PrustiCompilerCalls {
    fn config(&mut self, config: &mut Config) {
        assert!(config.override_queries.is_none());
        config.override_queries = Some(override_queries);
    }
    fn after_expansion<'tcx>(
        &mut self,
        compiler: &Compiler,
        queries: &'tcx Queries<'tcx>,
    ) -> Compilation {
        compiler.session().abort_if_errors();
        let (krate, _resolver, _lint_store) = &mut *queries.expansion().unwrap().peek_mut();
        if config::print_desugared_specs() {
            prusti_rustc_interface::driver::pretty::print_after_parsing(
                compiler.session(),
                compiler.input(),
                krate,
                prusti_rustc_interface::session::config::PpMode::Source(
                    prusti_rustc_interface::session::config::PpSourceMode::Normal,
                ),
                None,
            );
        }
        Compilation::Continue
    }
    fn after_analysis<'tcx>(
        &mut self,
        compiler: &Compiler,
        queries: &'tcx Queries<'tcx>,
    ) -> Compilation {
        compiler.session().abort_if_errors();
        queries.global_ctxt().unwrap().peek_mut().enter(|tcx| {
            let mut env = Environment::new(tcx);
            let spec_checker = specs::checker::SpecChecker::new();
            spec_checker.check(&env);
            compiler.session().abort_if_errors();

            let hir = env.query.hir();
            let mut spec_collector = specs::SpecCollector::new(&mut env);
            hir.walk_toplevel_module(&mut spec_collector);
            hir.walk_attributes(&mut spec_collector);

            let build_output_dir = compiler.output_dir();
            let def_spec = spec_collector.build_def_specs(build_output_dir);
            if config::print_typeckd_specs() {
                for value in def_spec.all_values_debug(config::hide_uuids()) {
                    println!("{}", value);
                }
            }
            if !config::no_verify() {
                verify(env, def_spec);
            }
        });

        compiler.session().abort_if_errors();
        if config::full_compilation() {
            Compilation::Continue
        } else {
            Compilation::Stop
        }
    }
}
