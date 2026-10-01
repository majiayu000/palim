use jsondiffpatch_rs::{Delta,patch,reverse};
use serde_json::{Value,json};
fn rng(s:&mut u64)->usize { *s^=*s<<13; *s^=*s>>7; *s^=*s<<17; (*s>>16) as usize }
fn main(){
 let mut seed=2026100103u64;
 for case in 0..100000usize {
  let n=rng(&mut seed)%51;
  let source:Vec<Value>=(0..n).map(|i|json!({"id":i,"v":i*17+3,"nested":{"a/b~":i}})).collect();
  let mut deleted=vec![false;n];let mut moved=vec![false;n];let mut survivors=Vec::new();let mut todo=Vec::new();
  for i in 0..n {match rng(&mut seed)%5 {0=>deleted[i]=true,1|2=>{moved[i]=true;todo.push(i);},_=>survivors.push(i)}}
  for i in (1..todo.len()).rev(){let j=rng(&mut seed)%(i+1);todo.swap(i,j);}
  let add_count=rng(&mut seed)%13;
  // The only protocol restriction on survivors is original relative order.
  let mut order:Vec<Option<usize>>=Vec::new();let mut a=0;let mut b=0;let mut c=0;
  while a<survivors.len() || b<todo.len() || c<add_count {
   let pick=rng(&mut seed)%3;
   if pick==0 && a<survivors.len(){order.push(Some(survivors[a]));a+=1;}
   else if pick==1 && b<todo.len(){order.push(Some(todo[b]));b+=1;}
   else if c<add_count {order.push(None);c+=1;}
  }
  let mut fields=serde_json::Map::new();fields.insert("_t".into(),json!("a"));
  for i in 0..n {if deleted[i]{fields.insert(format!("_{i}"),json!([source[i],0,0]));}}
  let mut target=Vec::new();
  for (j,item) in order.iter().enumerate() {
   if let Some(i)=item {
    if moved[*i] {
      let included=if rng(&mut seed)%2==0 {json!("")}else{source[*i].clone()};
      fields.insert(format!("_{i}"),json!([included,j,3]));
    }
    let mut val=source[*i].clone();
    if rng(&mut seed)%3==0 {
      let new=(rng(&mut seed)%999999) as u64;
      fields.insert(j.to_string(),json!({"v":[val["v"],new],"nested":{"a/b~":[*i,new]}}));
      val["v"]=json!(new);val["nested"]["a/b~"]=json!(new);
    }
    target.push(val);
   } else {
    let val=json!({"fresh":case*100+j,"nested":[true,null,{"x/y~":j}]});
    fields.insert(j.to_string(),json!([val]));target.push(val);
   }
  }
  let left=json!({"a/b~":source});let right=json!({"a/b~":target});
  let change=Delta::from_value(json!({"a/b~":Value::Object(fields)})).unwrap();
  assert_eq!(patch(&left,&change).unwrap(),right,"case {case}: {change:?}");
  assert_eq!(patch(&right,&reverse(&change).unwrap()).unwrap(),left,"inverse {case}");
  let exported=change.to_json_patch(&left).unwrap();
  let count=exported.0.iter().filter(|v|matches!(v,json_patch::PatchOperation::Move(_))).count();
  assert!(count<=moved.iter().filter(|&&v|v).count());
  let mut got=left.clone();json_patch::patch(&mut got,&exported).unwrap();
  assert_eq!(got,right,"export {case}: {change:?}: {exported:?}");
 }
 println!("100000 imported mixed-array deltas: native, inverse, external RFC apply and move bound passed; seed=2026100103");
}
