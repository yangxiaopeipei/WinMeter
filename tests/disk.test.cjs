const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function view() {
  const elements = Object.fromEntries(['disk-app-period','disk-app-body','disk-devices'].map((id) => [id,{innerHTML:'',value:'today',querySelectorAll:()=>[]} ]));
  const context={window:{},document:{getElementById:(id)=>elements[id]}};
  vm.runInNewContext(fs.readFileSync('src/disk.js','utf8'),context);
  return {elements,render:(sample)=>context.window.WinMeterDisk.render(sample,{bytes:(b)=>b+' B',cap:(b)=>b+' bytes'})};
}
test('health does not invent readings for unsupported fields or equate missing SMART with healthy',()=>{
  const v=view();v.render({physical_disks:[{id:'/0',number:0,name:'SSD',status:'Unknown',kind:'SSD',mounts:[]}]});
  const html=v.elements['disk-devices'].innerHTML;
  assert.match(html,/健康状态未提供/);assert.match(html,/设备未提供 SMART/);
  assert.doesNotMatch(html,/预估剩余寿命|累计主机写入|通电时间|0 °C/);
});
test('real zero remaining life is retained and device-provided warnings are visible',()=>{
  const v=view();v.render({physical_disks:[{id:'/0',number:0,name:'SSD',status:'Caution',life_percent:0,host_write_gb:1234,power_on_hours:0}]});
  const html=v.elements['disk-devices'].innerHTML;
  assert.match(html,/需要关注/);assert.match(html,/预估剩余寿命/);assert.match(html,/0%/);assert.match(html,/1,234 GB/);
});
test('period change uses the latest sample and app identities cannot inject markup',()=>{
  const v=view();const rows={today:[{name:'<script>',path:'C:\\a"<.exe',read:1,write:20}],week:[{name:'week.exe',read:5,write:100}]};
  v.render({app_io:rows});assert.match(v.elements['disk-app-body'].innerHTML,/&lt;script&gt;/);
  assert.doesNotMatch(v.elements['disk-app-body'].innerHTML,/<script>/);
  v.elements['disk-app-period'].value='week';v.elements['disk-app-period'].onchange();
  assert.match(v.elements['disk-app-body'].innerHTML,/week.exe/);assert.match(v.elements['disk-app-body'].innerHTML,/100 B/);
  v.render({app_io:{week:[]}});assert.match(v.elements['disk-app-body'].innerHTML,/首次采样只建立基线/);
});
