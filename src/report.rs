use errors::*;
use ex;
use file;
use gh_mirrors;
use handlebars::Handlebars;
use results::{CrateResultWriter, ExperimentResultDB, FileDB, TestResult};
use serde_json;
use std::{fs, io};
use std::fs::File;
use std::path::Path;

#[derive(Serialize, Deserialize)]
pub struct TestResults {
    crates: Vec<CrateResult>,
}

#[derive(Serialize, Deserialize)]
struct CrateResult {
    name: String,
    res: Comparison,
    runs: [Option<BuildTestResult>; 2],
}

#[derive(Serialize, Deserialize)]
enum Comparison {
    Regressed,
    Fixed,
    Unknown,
    SameBuildFail,
    SameTestFail,
    SameTestPass,
}

#[derive(Serialize, Deserialize)]
struct BuildTestResult {
    res: TestResult,
    log: String,
}


pub fn generate_report(ex: &ex::Experiment) -> Result<TestResults> {
    let db = FileDB::for_experiment(ex);
    assert_eq!(ex.toolchains.len(), 2);

    let res = ex::ex_crates_and_dirs(ex)?
        .into_iter()
        .map(|(krate, _)| {
            // Any errors here will turn into unknown results
            let crate_results = ex.toolchains
                .iter()
                .map(|tc| -> Result<BuildTestResult> {
                    let writer = db.for_crate(&krate, tc);
                    let res = writer.load_test_result()?;
                    // If there was no test result return an error
                    let res = res.ok_or_else(|| Error::from("no result"))?;
                    let rel_log = writer.result_path_fragement();

                    Ok(BuildTestResult {
                           res: res,
                           log: format!("{}", rel_log.display()),
                       })
                });
            // Convert errors to Nones
            let mut crate_results = crate_results.map(|r| r.ok()).collect::<Vec<_>>();
            let crate2 = crate_results.pop().expect("");
            let crate1 = crate_results.pop().expect("");
            let comp = compare(&crate1, &crate2);

            CrateResult {
                name: crate_to_name(&krate).unwrap_or_else(|_| "<unknown>".into()),
                res: comp,
                runs: [crate1, crate2],
            }
        })
        .collect::<Vec<_>>();

    Ok(TestResults { crates: res })
}

pub fn write_logs(ex: &ex::Experiment, dest: &Path) -> Result<()> {
    let db = FileDB::for_experiment(ex);
    for (krate, _) in ex::ex_crates_and_dirs(ex)? {
        for tc in &ex.toolchains {
            let writer = db.for_crate(&krate, tc);
            let rel_log = writer.result_path_fragement();

            let mut result_log = writer.read_log()?;
            write_log_file(dest, &rel_log, &mut result_log)?;
        }
    }
    Ok(())
}


pub fn gen(ex_name: &str, dest: &Path) -> Result<()> {
    let ex = ex::Experiment::load(ex_name)?;

    let res = generate_report(&ex)?;
    let shas = ex.load_shas()?;

    info!("writing results to {:?}", dest);
    file::write_string(&dest.join("results.json"), &serde_json::to_string(&res)?)?;
    file::write_string(&dest.join("config.json"), &serde_json::to_string(&ex)?)?;
    file::write_string(&dest.join("shas.json"), &serde_json::to_string(&shas)?)?;

    write_logs(&ex, dest)?;
    write_html_files(dest)?;

    Ok(())
}

fn crate_to_name(c: &ex::ExCrate) -> Result<String> {
    match *c {
        ex::ExCrate::Version {
            ref name,
            ref version,
        } => Ok(format!("{}-{}", name, version)),
        ex::ExCrate::Repo { ref url, ref sha } => {
            let (org, name) = gh_mirrors::gh_url_to_org_and_name(url)?;
            Ok(format!("{}.{}.{}", org, name, sha))
        }
    }
}

fn compare(r1: &Option<BuildTestResult>, r2: &Option<BuildTestResult>) -> Comparison {
    use results::TestResult::*;
    match (r1, r2) {
        (&Some(BuildTestResult { res: ref res1, .. }),
         &Some(BuildTestResult { res: ref res2, .. })) => {
            match (res1, res2) {
                (&BuildFail, &BuildFail) => Comparison::SameBuildFail,
                (&TestFail, &TestFail) => Comparison::SameTestFail,
                (&TestPass, &TestPass) => Comparison::SameTestPass,
                (&BuildFail, &TestFail) |
                (&BuildFail, &TestPass) |
                (&TestFail, &TestPass) => Comparison::Fixed,
                (&TestPass, &TestFail) |
                (&TestPass, &BuildFail) |
                (&TestFail, &BuildFail) => Comparison::Regressed,
            }
        }
        _ => Comparison::Unknown,
    }
}

#[derive(Serialize, Deserialize)]
pub struct Context {
    pub config_url: String,
    pub results_url: String,
    pub static_url: String,
}


fn write_html_files(dir: &Path) -> Result<()> {
    let html_in = include_str!("../template/report.html");
    let js_in = include_str!("../static/report.js");
    let css_in = include_str!("../static/report.css");
    let html_out = dir.join("index.html");
    let js_out = dir.join("report.js");
    let css_out = dir.join("report.css");

    info!("writing report to {}", html_out.display());

    let context = Context {
        config_url: "config.json".into(),
        results_url: "results.json".into(),
        static_url: "".into(),
    };
    let html = Handlebars::new()
        .template_render(html_in, &context)
        .chain_err(|| "Couldn't render template")?;

    file::write_string(&html_out, &html)?;
    file::write_string(&js_out, js_in)?;
    file::write_string(&css_out, css_in)?;

    Ok(())
}

fn write_log_file<R: io::Read>(dest: &Path, fragment: &Path, log: &mut R) -> Result<()> {
    let log_dir = dest.join(fragment);
    let log_file = log_dir.join("log.txt");
    fs::create_dir_all(log_dir)?;
    io::copy(log, &mut File::create(log_file)?)?;
    Ok(())
}
