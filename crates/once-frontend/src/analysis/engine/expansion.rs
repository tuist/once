use super::*;

impl AnalysisEngine {
    pub fn expand_actions(
        &self,
        target: &GraphTarget,
        workspace: &Path,
        action: &DeclaredAction,
    ) -> Result<AnalysisResult> {
        let super::super::DeclaredActionOperation::ExpandActions {
            implementation,
            args,
            build_dir,
        } = action
            .operation
            .as_ref()
            .context("missing action expansion")?
        else {
            anyhow::bail!("expected an action expansion");
        };
        let store = AnalysisStore::with_host_cache(
            workspace.to_path_buf(),
            target.label.package.clone(),
            build_dir.clone(),
            self.host_cache.clone(),
        );
        let (store, result) = with_active_store(store, || {
            Module::with_temp_heap(|module| {
                let callback = self
                    .module
                    .get(implementation)
                    .with_context(|| format!("reading action planner `{implementation}`"))?;
                let callback = module.heap().access_owned_frozen_value(&callback);
                let mut eval = Evaluator::new(&module);
                let context = serde_json::json!({
                    "args": args,
                    "inputs": action.inputs,
                    "outputs": action.outputs,
                    "build_dir": build_dir,
                    "label": {"id": target.label.id, "package": target.label.package},
                });
                let context = json_to_value(&eval, &context);
                let returned = eval
                    .eval_function(callback, &[context], &[])
                    .map_err(|error| {
                        analysis_failure(target, "implementation", &error.to_string())
                    })?;
                if !returned.is_none() {
                    let diagnostic: Diagnostic = serde_json::from_value(value_to_json(returned))
                        .context("action planner must return None or a structured diagnostic")?;
                    return Err(AnalysisFailure { diagnostic }.into());
                }
                Ok::<_, anyhow::Error>(())
            })
        });
        result?;
        if store.actions.iter().any(|action| {
            matches!(
                action.operation,
                Some(super::super::DeclaredActionOperation::ExpandActions { .. })
            )
        }) {
            anyhow::bail!("action planners cannot recursively expand actions");
        }
        for output in &action.outputs {
            if !store
                .actions
                .iter()
                .any(|action| action.outputs.contains(output))
            {
                anyhow::bail!(
                    "action planner `{implementation}` did not declare promised output `{output}`"
                );
            }
        }
        Ok(AnalysisResult {
            actions: store.actions,
            provider: JsonValue::Null,
            declared_outputs: store.declared_outputs,
            observations: store.observations,
        })
    }
}
