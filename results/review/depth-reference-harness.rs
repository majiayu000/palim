use jsondiffpatch_rs::{Patch,PatchOperation,Error,JsonPatchApplyOptions,apply_json_patch_with_options};
use serde_json::{Value,json};
fn rng(s:&mut u64)->usize { *s^=*s<<13; *s^=*s>>7; *s^=*s<<17; (*s>>16) as usize }
fn height(v:&Value)->usize {match v{Value::Array(a)=>1+a.iter().map(height).max().unwrap_or(0),Value::Object(o)=>1+o.values().map(height).max().unwrap_or(0),_=>0}}
fn value(s:&mut u64,depth:usize)->Value {if depth==0 || rng(s)%3==0{return json!((rng(s)%7) as u64)};if rng(s)%2==0 {json!([value(s,depth-1),value(s,depth-1)])} else{json!({"x":value(s,depth-1),"a/b~":value(s,depth-1)})}}
fn error(i:usize,path:&str,msg:&str)->Error{Error{path:path.to_string(),message:format!("operation '/{i}' failed at path '{path}': {msg}")}}
fn reference(left:&Value,p:&Patch,o:&JsonPatchApplyOptions)->Result<Value,Error>{
 if height(left)>o.max_depth {return Err(Error{path:"".into(),message:"JSON nesting exceeds max_depth".into()})}
 let mut doc=left.clone();let mut bytes=0;
 for (i,op) in p.0.iter().enumerate(){
  let path=match op {PatchOperation::Add(o)=>o.path.as_str(),PatchOperation::Remove(o)=>o.path.as_str(),PatchOperation::Replace(o)=>o.path.as_str(),PatchOperation::Move(o)=>o.path.as_str(),PatchOperation::Copy(o)=>o.path.as_str(),PatchOperation::Test(o)=>o.path.as_str()};
  if let (Some(limit),PatchOperation::Copy(copy))=(o.max_copy_bytes,op){
   let v=doc.pointer(copy.from.as_str()).ok_or_else(||error(i,path,"\"from\" path is invalid"))?;
   bytes+=serde_json::to_vec(v).unwrap().len();
   if bytes>limit{return Err(error(i,path,"copy byte budget exceeded"))}
  }
  let payload=match op{PatchOperation::Add(op)=>Some(&op.value),PatchOperation::Replace(op)=>Some(&op.value),PatchOperation::Test(op)=>Some(&op.value),_=>None};
  if payload.is_some_and(|v|height(v)>o.max_depth){return Err(error(i,path,"JSON nesting exceeds max_depth"))}
  if let PatchOperation::Test(op)=op {
   let got=doc.pointer(path).ok_or_else(||error(i,path,"path is invalid"))?;
   if got!=&op.value{return Err(error(i,path,"value did not match"))};continue;
  }
  if matches!(op,PatchOperation::Move(op) if op.from.as_str().is_empty() && op.path.as_str().is_empty()){continue}
  json_patch::patch(&mut doc,&Patch(vec![op.clone()])).map_err(|mut e|{e.operation=i;Error{path:e.path.to_string(),message:e.to_string()}})?;
  if height(&doc)>o.max_depth{return Err(error(i,path,"JSON nesting exceeds max_depth"))}
 }
 Ok(doc)
}
fn main(){
 let paths=["","/x","/a~1b~0","/x/x","/x/0","/x/-","/items/0","/items/1","/items/2","/items/-","/missing/child","/source","/source/leaf","/target/new","/items/0/x","/items/00","/target"];
 let mut seed=2026100104u64;
 for case in 0..100000usize {
  let depth=rng(&mut seed)%5;
  let left=if rng(&mut seed)%5==0 {value(&mut seed,depth)}else {json!({"x":value(&mut seed,depth),"source":{"leaf":value(&mut seed,depth)},"items":[value(&mut seed,depth),value(&mut seed,depth)],"target":{},"a/b~":value(&mut seed,depth)})};
  let mut ops=Vec::new();let count=rng(&mut seed)%11;
  for _ in 0..count {
   let path=paths[rng(&mut seed)%paths.len()];let from=paths[rng(&mut seed)%paths.len()];let d=rng(&mut seed)%5;
   ops.push(match rng(&mut seed)%6 {0=>json!({"op":"add","path":path,"value":value(&mut seed,d)}),1=>json!({"op":"replace","path":path,"value":value(&mut seed,d)}),2=>json!({"op":"remove","path":path}),3=>json!({"op":"move","from":from,"path":path}),4=>json!({"op":"copy","from":from,"path":path}),_=>json!({"op":"test","path":path,"value":value(&mut seed,d)})});
  }
  let p:Patch=serde_json::from_value(json!(ops)).unwrap();
  let options=JsonPatchApplyOptions{max_depth:rng(&mut seed)%7,max_copy_bytes:if rng(&mut seed)%2==0{Some(rng(&mut seed)%100)}else{None}};
  let actual=apply_json_patch_with_options(&left,&p,&options);let wanted=reference(&left,&p,&options);
  assert_eq!(actual,wanted,"case {case}, left={left}, patch={p:?}, options={options:?}");
 }
 println!("100000 standard sequences: local depth outcome, complete result and exact error path/message agree with full-scan reference; seed=2026100104");
}
